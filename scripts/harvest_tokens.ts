#!/usr/bin/env bun
/**
 * Lazy On-Demand Token Harvester for Russian Marketplaces (Wildberries, Ozon).
 * Resolves Wbaas PoW challenges in a Bun JS environment and outputs JSON to stdout.
 *
 * Output format:
 * {
 *   "marketplace": "wildberries" | "ozon",
 *   "cookies": string,
 *   "expires_at": number, // unix timestamp in seconds
 *   "user_agent": string
 * }
 */

import { parseArgs } from "util";

const DEFAULT_USER_AGENT =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

interface HarvestedSession {
  marketplace: string;
  cookies: string;
  expires_at: number;
  user_agent: string;
}

function parseTokenExpiry(token: string): number {
  try {
    const parts = token.split(".");
    if (parts.length >= 4) {
      const b64 = parts[3];
      const decoded = Buffer.from(b64, "base64").toString("utf-8");
      // Format: 100|IP|UA|EXPIRE_TS|reusable|...
      const fields = decoded.split("|");
      if (fields.length >= 4) {
        const exp = parseInt(fields[3], 10);
        if (!isNaN(exp) && exp > 0) {
          return exp;
        }
      }
    }
  } catch {
    // Fall back to default TTL
  }
  return Math.floor(Date.now() / 1000) + 1209600;
}

async function harvestWildberries(userAgent: string): Promise<HarvestedSession> {
  const baseHeaders = {
    "User-Agent": userAgent,
    Accept:
      "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
    "Accept-Language": "ru-RU,ru;q=0.9,en-US;q=0.8,en;q=0.7",
    "sec-ch-ua":
      '"Chromium";v="124", "Google Chrome";v="124", "Not-A.Brand";v="99"',
    "sec-ch-ua-mobile": "?0",
    "sec-ch-ua-platform": '"Linux"',
  };

  // 1. Initial request to www.wildberries.ru to check status and extract siteKey
  let siteKey = "7400bd5df8b843b28254659f10915f31";
  try {
    const initRes = await fetch("https://www.wildberries.ru/", {
      headers: baseHeaders,
    });
    const html = await initRes.text();
    const siteKeyMatch = html.match(/data-site-key=["']([a-f0-9]+)["']/i);
    if (siteKeyMatch && siteKeyMatch[1]) {
      siteKey = siteKeyMatch[1];
    }
  } catch (err) {
    console.error(`[harvest_tokens] Failed to fetch initial WB page: ${err}`);
  }

  // 2. Request challenge from Wbaas antibot create-token API
  const createTokenUrl =
    "https://www.wildberries.ru/__wbaas/challenges/antibot/api/v1/create-token";
  const apiHeaders = {
    "User-Agent": userAgent,
    "Content-Type": "application/json",
    Origin: "https://www.wildberries.ru",
    Referer: "https://www.wildberries.ru/",
    "X-Guardium-Antibot-Key": siteKey,
    "X-Guardium-Antibot-Sdk-Version": "js-front-browser/3.1.0",
    "sec-ch-ua":
      '"Chromium";v="124", "Google Chrome";v="124", "Not-A.Brand";v="99"',
    "sec-ch-ua-mobile": "?0",
    "sec-ch-ua-platform": '"Linux"',
  };

  const initialReq = await fetch(createTokenUrl, {
    method: "POST",
    headers: apiHeaders,
    body: JSON.stringify({}),
  });

  let secureToken: string | null = null;

  if (initialReq.status === 200) {
    const data = (await initialReq.json()) as { secureToken?: string };
    secureToken = data.secureToken ?? null;
  } else if (initialReq.status === 498) {
    const challengeData = (await initialReq.json()) as {
      challenge?: { scriptPath: string; payload: string };
    };
    if (!challengeData.challenge) {
      throw new Error(
        `Wbaas returned 498 without challenge structure: ${JSON.stringify(challengeData)}`
      );
    }

    const { scriptPath, payload } = challengeData.challenge;
    const scriptUrl = `https://www.wildberries.ru/__wbaas/challenges/antibot${scriptPath}`;
    const scriptRes = await fetch(scriptUrl, {
      headers: {
        "User-Agent": userAgent,
        Referer: "https://www.wildberries.ru/",
      },
    });

    if (!scriptRes.ok) {
      throw new Error(
        `Failed to fetch challenge script from ${scriptUrl}: HTTP ${scriptRes.status}`
      );
    }

    const scriptCode = await scriptRes.text();

    // Setup browser DOM environment for challenge script
    const globalObj = globalThis as unknown as Record<string, unknown>;
    globalObj.window = globalThis;
    globalObj.document = {
      createElement: () => ({
        appendChild: () => {},
        setAttribute: () => {},
      }),
      head: {},
      body: {},
      addEventListener: () => {},
      documentElement: { lang: "ru" },
    };
    globalObj.navigator = {
      userAgent,
      languages: ["ru-RU", "ru", "en-US", "en"],
      language: "ru-RU",
      platform: "Linux x86_64",
    };

    // Execute challenge script to register solver symbol
    eval(scriptCode);

    const symbols = Object.getOwnPropertySymbols(globalThis);
    if (symbols.length === 0) {
      throw new Error("Challenge script evaluated but no global symbols found");
    }

    // Solver function is attached via the registered symbol
    type SolverFn = (p: string) => Promise<string> | string;
    const solverFn = (globalThis as unknown as Record<symbol, SolverFn>)[
      symbols[0]
    ];
    if (typeof solverFn !== "function") {
      throw new Error("Symbol target is not a function");
    }

    // Solve challenge
    const solutionPayload = await solverFn(payload);

    // Submit solution back to create-token
    const solveReq = await fetch(createTokenUrl, {
      method: "POST",
      headers: apiHeaders,
      body: JSON.stringify({
        challenge: challengeData.challenge,
        solution: {
          payload: solutionPayload,
        },
      }),
    });

    if (!solveReq.ok) {
      const errText = await solveReq.text();
      throw new Error(
        `Failed to submit challenge solution: HTTP ${solveReq.status} - ${errText}`
      );
    }

    const solveData = (await solveReq.json()) as { secureToken?: string };
    secureToken = solveData.secureToken ?? null;
  } else {
    const errText = await initialReq.text();
    throw new Error(
      `Unexpected response from create-token: HTTP ${initialReq.status} - ${errText}`
    );
  }

  if (!secureToken) {
    throw new Error("No secureToken received from Wbaas challenge solver");
  }

  const expiry = parseTokenExpiry(secureToken);
  const uid = `${Math.floor(Date.now() / 1000)}${Math.floor(Math.random() * 900000000 + 100000000)}`;
  const cookies = `x_wbaas_token=${secureToken}; __wbaas_token=${secureToken}; _wbauid=${uid}`;

  return {
    marketplace: "wildberries",
    cookies,
    expires_at: expiry,
    user_agent: userAgent,
  };
}

async function harvestOzon(userAgent: string): Promise<HarvestedSession> {
  const baseHeaders = {
    "User-Agent": userAgent,
    Accept:
      "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
    "Accept-Language": "ru-RU,ru;q=0.9,en-US;q=0.8,en;q=0.7",
    "sec-ch-ua":
      '"Chromium";v="124", "Google Chrome";v="124", "Not-A.Brand";v="99"',
    "sec-ch-ua-mobile": "?0",
    "sec-ch-ua-platform": '"Linux"',
  };

  const collectedCookies: Record<string, string> = {};

  for (const url of [
    "https://www.ozon.ru/",
    "https://www.ozon.ru/api/composer-api.bx/page/json/v2?url=%2F",
  ]) {
    try {
      const res = await fetch(url, {
        headers: baseHeaders,
        redirect: "manual",
      });
      const cookieHeader = res.headers.get("set-cookie");
      if (cookieHeader) {
        for (const item of cookieHeader.split(/,\s*(?=[^;]+=[^;]+)/)) {
          const parts = item.split(";")[0]?.trim();
          if (parts) {
            const [k, v] = parts.split("=");
            if (k && v) {
              collectedCookies[k.trim()] = v.trim();
            }
          }
        }
      }
    } catch {
      // Continue collecting
    }
  }

  const cookieList = Object.entries(collectedCookies).map(
    ([k, v]) => `${k}=${v}`
  );

  return {
    marketplace: "ozon",
    cookies: cookieList.join("; "),
    expires_at: Math.floor(Date.now() / 1000) + 86400, // 24h TTL
    user_agent: userAgent,
  };
}

async function main() {
  const { values } = parseArgs({
    args: Bun.argv.slice(2),
    options: {
      marketplace: { type: "string", short: "m", default: "wildberries" },
      "user-agent": { type: "string", default: DEFAULT_USER_AGENT },
    },
    strict: true,
    allowPositionals: false,
  });

  const marketplace = (values.marketplace ?? "wildberries").toLowerCase();
  const userAgent = values["user-agent"] ?? DEFAULT_USER_AGENT;

  let session: HarvestedSession;
  if (marketplace === "wildberries" || marketplace === "wb") {
    session = await harvestWildberries(userAgent);
  } else if (marketplace === "ozon") {
    session = await harvestOzon(userAgent);
  } else {
    console.error(`Unknown marketplace: ${marketplace}`);
    process.exit(1);
  }

  // Pure JSON to stdout for Rust caller to parse
  process.stdout.write(JSON.stringify(session) + "\n");
  process.exit(0);
}

main().catch((err) => {
  console.error(`[harvest_tokens ERROR] ${err.message || err}`);
  process.exit(1);
});
