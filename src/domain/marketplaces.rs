use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Supported Russian and international marketplaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Marketplace {
    Wildberries,
    Ozon,
    YandexMarket,
    Dns,
    Citilink,
    Avito,
    AliExpress,
    Taobao,
    Megamarket,
    Lamoda,
    Detmir,
    Cian,
}

impl Marketplace {
    /// Return the canonical string identifier for the marketplace.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Wildberries => "wildberries",
            Self::Ozon => "ozon",
            Self::YandexMarket => "yandex_market",
            Self::Dns => "dns",
            Self::Citilink => "citilink",
            Self::Avito => "avito",
            Self::AliExpress => "aliexpress",
            Self::Taobao => "taobao",
            Self::Megamarket => "megamarket",
            Self::Lamoda => "lamoda",
            Self::Detmir => "detmir",
            Self::Cian => "cian",
        }
    }

    /// List of all supported marketplaces.
    pub const ALL: &'static [Self] = &[
        Self::Wildberries,
        Self::Ozon,
        Self::YandexMarket,
        Self::Dns,
        Self::Citilink,
        Self::Avito,
        Self::AliExpress,
        Self::Taobao,
        Self::Megamarket,
        Self::Lamoda,
        Self::Detmir,
        Self::Cian,
    ];
}

impl fmt::Display for Marketplace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for Marketplace {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_lowercase().replace('-', "_");
        match normalized.as_str() {
            "wildberries" | "wb" => Ok(Self::Wildberries),
            "ozon" => Ok(Self::Ozon),
            "yandex_market" | "yandex" | "ym" => Ok(Self::YandexMarket),
            "dns" | "dns_shop" => Ok(Self::Dns),
            "citilink" => Ok(Self::Citilink),
            "avito" => Ok(Self::Avito),
            "aliexpress" | "ali" => Ok(Self::AliExpress),
            "taobao" => Ok(Self::Taobao),
            "megamarket" | "sbermegamarket" => Ok(Self::Megamarket),
            "lamoda" => Ok(Self::Lamoda),
            "detmir" | "detsky_mir" => Ok(Self::Detmir),
            "cian" => Ok(Self::Cian),
            _ => Err(format!("Unknown marketplace: '{s}'")),
        }
    }
}

/// Marketplace-specific co-branded or virtual bank cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardType {
    OzonCard,
    WbWallet,
    SberSpasibo,
    YandexPay,
}

impl fmt::Display for CardType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OzonCard => write!(f, "ozon_card"),
            Self::WbWallet => write!(f, "wb_wallet"),
            Self::SberSpasibo => write!(f, "sber_spasibo"),
            Self::YandexPay => write!(f, "yandex_pay"),
        }
    }
}

impl FromStr for CardType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().replace('-', "_").as_str() {
            "ozon_card" | "ozon" => Ok(Self::OzonCard),
            "wb_wallet" | "wb" => Ok(Self::WbWallet),
            "sber_spasibo" | "spasibo" => Ok(Self::SberSpasibo),
            "yandex_pay" | "ypay" => Ok(Self::YandexPay),
            _ => Err(format!("Unknown card type: '{s}'")),
        }
    }
}

/// Loyalty subscription programs offering discount tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionType {
    YandexPlus,
    SberPrime,
    OzonPremium,
}

impl fmt::Display for SubscriptionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::YandexPlus => write!(f, "yandex_plus"),
            Self::SberPrime => write!(f, "sber_prime"),
            Self::OzonPremium => write!(f, "ozon_premium"),
        }
    }
}

impl FromStr for SubscriptionType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().replace('-', "_").as_str() {
            "yandex_plus" | "plus" => Ok(Self::YandexPlus),
            "sber_prime" | "prime" => Ok(Self::SberPrime),
            "ozon_premium" | "premium" => Ok(Self::OzonPremium),
            _ => Err(format!("Unknown subscription type: '{s}'")),
        }
    }
}

/// ISO currency identifiers for offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    Rub,
    Cny,
    Usd,
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rub => write!(f, "rub"),
            Self::Cny => write!(f, "cny"),
            Self::Usd => write!(f, "usd"),
        }
    }
}

impl FromStr for Currency {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "rub" | "ruble" | "rubles" | "₽" => Ok(Self::Rub),
            "cny" | "yuan" | "rmb" | "¥" => Ok(Self::Cny),
            "usd" | "dollar" | "dollars" | "$" => Ok(Self::Usd),
            _ => Err(format!("Unknown currency: '{s}'")),
        }
    }
}

/// Product stock availability and quantity status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StockStatus {
    InStock(u32),
    OutOfStock,
    PreOrder,
    Unknown,
}

impl StockStatus {
    /// Returns true if the product is available to purchase now.
    pub const fn is_in_stock(&self) -> bool {
        match self {
            Self::InStock(qty) => *qty > 0,
            Self::PreOrder => true,
            Self::OutOfStock | Self::Unknown => false,
        }
    }

    /// Return available quantity if known.
    pub const fn quantity(&self) -> Option<u32> {
        match self {
            Self::InStock(qty) => Some(*qty),
            _ => None,
        }
    }
}

/// Anti-bot and WAF protection systems encountered on marketplaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WafProtection {
    Wbaas,
    Kasada,
    CloudflareTurnstile,
    Variti,
    None,
}

impl fmt::Display for WafProtection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wbaas => write!(f, "Wbaas (Wildberries)"),
            Self::Kasada => write!(f, "Kasada / Qrator Anti-Bot"),
            Self::CloudflareTurnstile => write!(f, "Cloudflare Turnstile"),
            Self::Variti => write!(f, "Variti (Ozon Anti-Bot)"),
            Self::None => write!(f, "None"),
        }
    }
}
