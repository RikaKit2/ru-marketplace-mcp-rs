pub mod aggregator;
pub mod verification;

pub use aggregator::{AggregatorService, CompareResult, SourceOutcome};
pub use verification::{OfferVerificationResult, verify_offer};
