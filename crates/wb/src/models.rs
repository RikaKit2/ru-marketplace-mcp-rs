use serde::{Deserialize, Serialize};

/// Root model line info for a Wildberries SKU (from basket CDN card.json).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WbRootInfo {
    /// Root product id (imt_id) shared by all variants, required for reviews.
    pub imt_id: u64,
    /// Category/subject name.
    pub subj_name: String,
    /// Root subject name.
    pub subj_root_name: String,
    /// Vendor code / article if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_code: Option<String>,
    /// Product description text if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Product specifications / options list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<serde_json::Value>,
    /// Basket CDN host used to fetch this record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_used: Option<String>,
}

/// A single buyer review on Wildberries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WbReviewItem {
    /// Review star rating (1..=5).
    pub rating: Option<u8>,
    /// Review comment / body text.
    pub text: String,
    /// Stated pros / advantages.
    pub pros: String,
    /// Stated cons / disadvantages.
    pub cons: String,
    /// Reviewer user name.
    pub user: String,
    /// Review creation timestamp (ISO 8601).
    pub date: Option<String>,
}

/// Reviews response containing review pool and metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WbReviewsResponse {
    /// Root product id (imt_id) the reviews belong to.
    pub imt_id: u64,
    /// Total review / feedback count reported by Wildberries.
    pub feedback_count: Option<u64>,
    /// Overall valuation / rating (e.g. "4.8").
    pub valuation: Option<String>,
    /// List of review items for the requested page.
    pub feedbacks: Vec<WbReviewItem>,
    /// Page number (1-based).
    pub page: usize,
    /// Total pages available.
    pub total_pages: usize,
    /// CDN host used to fetch reviews.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_used: Option<String>,
}

/// Registered legal entity information for a Wildberries seller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WbSellerInfo {
    /// Wildberries supplier ID.
    pub supplier_id: u64,
    /// Seller trading / brand name (supplierName).
    pub supplier_name: String,
    /// Full registered legal entity name (supplierFullName).
    pub full_name: Option<String>,
    /// Russian taxpayer ID (INN).
    pub inn: Option<String>,
    /// State registration number (OGRN / OGRNIP).
    pub ogrn: Option<String>,
    /// Registered legal address.
    pub legal_address: Option<String>,
    /// Registered trademark if specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trademark: Option<String>,
    /// Tax registration reason code (KPP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kpp: Option<String>,
    /// CDN host used to fetch seller data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_used: Option<String>,
}
