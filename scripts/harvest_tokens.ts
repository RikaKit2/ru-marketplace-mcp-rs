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
import fs from "fs";
import puppeteer from "puppeteer-core";

const PROFILE_DIR = "/tmp/ozon-harvester-profile";

function clearStaleProfileLock(dir: string): void {
  for (const lock of ["SingletonLock", "SingletonCookie", "SingletonSocket"]) {
    try {
      fs.unlinkSync(`${dir}/${lock}`);
    } catch {}
  }
}

function sleep(ms: number): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  setTimeout(resolve, ms);
  return promise;
}

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
  const chromiumPath =
    process.env.CHROMIUM_PATH || "/run/current-system/sw/bin/chromium";

  clearStaleProfileLock(PROFILE_DIR);
  const browser = await puppeteer.launch({
    executablePath: chromiumPath,
    headless: false,
    args: [
      "--disable-blink-features=AutomationControlled",
      "--no-sandbox",
      "--disable-dev-shm-usage",
      `--user-data-dir=${PROFILE_DIR}`,
    ],
  });

  try {
    const page = await browser.newPage();
    await page.setUserAgent(userAgent);
    await page.goto("https://www.ozon.ru/", {
      waitUntil: "domcontentloaded",
      timeout: 30000,
    });
    // Wait for Variti challenge to settle and set all tokens
    await sleep(6000);

    const cookies = await page.cookies();
    const cookieList = cookies.map(
      (c: { name: string; value: string }) => `${c.name}=${c.value}`
    );

    if (cookieList.length === 0) {
      throw new Error("No cookies collected from Ozon challenge");
    }

    return {
      marketplace: "ozon",
      cookies: cookieList.join("; "),
      expires_at: Math.floor(Date.now() / 1000) + 86400, // 24h TTL
      user_agent: userAgent,
    };
  } finally {
    browser.process()?.kill("SIGKILL");
  }
}
async function fetchOzonComposer(
  urlParam: string,
  userAgent: string
): Promise<string> {
  const chromiumPath =
    process.env.CHROMIUM_PATH || "/run/current-system/sw/bin/chromium";

  clearStaleProfileLock(PROFILE_DIR);
  const browser = await puppeteer.launch({
    executablePath: chromiumPath,
    headless: false,
    args: [
      "--disable-blink-features=AutomationControlled",
      "--no-sandbox",
      "--disable-dev-shm-usage",
      `--user-data-dir=${PROFILE_DIR}`,
    ],
  });

  try {
    const page = await browser.newPage();
    await page.setUserAgent(userAgent);
    await page.goto("https://www.ozon.ru/", {
      waitUntil: "domcontentloaded",
      timeout: 30000,
    });
    // Wait for Variti challenge to settle
    await sleep(5000);

    const jsonText = await page.evaluate(async (endpointUrl: string) => {
      const fullUrl =
        "/api/composer-api.bx/page/json/v2?url=" +
        encodeURIComponent(endpointUrl);
      const res = await fetch(fullUrl, {
        headers: { accept: "application/json" },
        credentials: "include",
      });
      return await res.text();
    }, urlParam);

    return jsonText;
  } finally {
    browser.process()?.kill("SIGKILL");
  }
}

async function main() {
  const { values } = parseArgs({
    args: Bun.argv.slice(2),
    options: {
      marketplace: { type: "string", short: "m", default: "wildberries" },
      "user-agent": { type: "string", default: DEFAULT_USER_AGENT },
      "fetch-composer": { type: "string" },
    },
    strict: true,
    allowPositionals: false,
  });

  if (values["fetch-composer"]) {
    const jsonText = await fetchOzonComposer(
      values["fetch-composer"],
      values["user-agent"] ?? DEFAULT_USER_AGENT
    );
    const cacheDir = "/home/user/.cache/ru-marketplace-mcp";
    fs.mkdirSync(cacheDir, { recursive: true });
    const outFile = `${cacheDir}/composer_${Date.now()}_${Math.floor(Math.random() * 10000)}.json`;
    fs.writeFileSync(outFile, jsonText);
    process.stdout.write(outFile);
    process.exit(0);
  }

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
