use ru_marketplace_core::domain::ProductOffer;
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

/// One node of the Wildberries catalog tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WbCategoryNode {
    /// Category numeric ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    /// Category display name.
    pub name: String,
    /// Site-relative category URL.
    pub url: String,
    /// WB catalog shard name (needed to query this category's feed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shard: Option<String>,
    /// WB catalog query selector (cat= or subject=).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Depth in the tree slice (0 for root items of the slice).
    #[serde(default)]
    pub depth: usize,
    /// Total count of direct child categories.
    #[serde(default)]
    pub children_count: usize,
    /// Direct child categories if expanded within depth limit.
    #[serde(default, alias = "children", skip_serializing_if = "Vec::is_empty")]
    pub childs: Vec<WbCategoryNode>,
}

impl WbCategoryNode {
    /// Direct child categories.
    pub fn children(&self) -> &[WbCategoryNode] {
        &self.childs
    }
}

/// Bounded slice of the Wildberries catalog category tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WbCategoriesResponse {
    /// Requested or resolved root category name.
    pub root: String,
    /// Maximum depth applied to this response.
    pub max_depth: usize,
    /// Total category nodes returned in this slice.
    pub total_returned: usize,
    /// Whether the slice was truncated due to node budget limits.
    pub truncated: bool,
    /// Category nodes at the requested root.
    pub items: Vec<WbCategoryNode>,
    /// Static CDN host that served the menu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_used: Option<String>,
}

/// A page of products from one Wildberries catalog category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WbCategoryProductsResponse {
    /// WB catalog shard used.
    pub shard: String,
    /// WB catalog query selector used.
    pub query: String,
    /// Page number (1-based).
    pub page: usize,
    /// Sorting applied (e.g. "popular", "priceup", etc.).
    pub sort: String,
    /// WB region destination ID used.
    pub dest: String,
    /// Number of items on this page.
    pub count: usize,
    /// Whether more pages likely follow (true if page was full).
    pub has_more: bool,
    /// Product offers on this category page.
    pub items: Vec<ProductOffer>,
}
