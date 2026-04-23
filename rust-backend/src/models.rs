//! HTTP-facing models mapped from SQL rows.
//!
//! Cron HTTP responses are also summarized in the `cron_runs` audit table (see [`CronRun`]).
//! **`Invoice.metadata`** is JSONB for extensibility. It is not used in SQL filters in the
//! current codebase; indexing follows the plan in `../usdc-payment-link-tool/migrations/003_invoice_metadata_jsonb_index_plan.sql`.
//! Merchant sessions are persisted in the `sessions` table (not represented as a struct here). Storage layout and indexes are defined under
//! `../usdc-payment-link-tool/migrations/`; see [`crate::auth::current_merchant`] and [`crate::db`] for query assumptions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_postgres::Row;
use uuid::Uuid;

/// One row per `/api/cron/reconcile` or `/api/cron/settle` invocation that reached the handler body.
/// Used when listing audit rows from SQL; no dedicated HTTP route yet.
#[allow(dead_code)]
#[derive(Clone, Serialize)]
pub struct CronRun {
    pub id: Uuid,
    #[serde(rename = "jobType")]
    pub job_type: String,
    #[serde(rename = "startedAt")]
    pub started_at: DateTime<Utc>,
    #[serde(rename = "finishedAt")]
    pub finished_at: DateTime<Utc>,
    pub success: bool,
    pub metadata: Value,
    #[serde(rename = "errorDetail")]
    pub error_detail: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct Merchant {
    pub id: Uuid,
    pub email: String,
    pub business_name: String,
    pub stellar_public_key: String,
    pub settlement_public_key: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Serialize)]
pub struct LoginMerchant {
    pub id: Uuid,
    pub email: String,
    #[serde(rename = "businessName")]
    pub business_name: String,
}

#[derive(Clone, Serialize)]
pub struct Invoice {
    pub id: Uuid,
    pub public_id: String,
    pub merchant_id: Uuid,
    pub description: String,
    pub amount_cents: i32,
    pub currency: String,
    pub asset_code: String,
    pub asset_issuer: String,
    pub destination_public_key: String,
    pub memo: String,
    pub status: String,
    pub gross_amount_cents: i32,
    pub platform_fee_cents: i32,
    pub net_amount_cents: i32,
    pub expires_at: DateTime<Utc>,
    pub paid_at: Option<DateTime<Utc>>,
    pub settled_at: Option<DateTime<Utc>>,
    pub transaction_hash: Option<String>,
    pub settlement_hash: Option<String>,
    pub checkout_url: Option<String>,
    pub qr_data_url: Option<String>,
    /// Opaque JSONB; add DB indexes only when queries filter on documented keys (see migrations).
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    #[serde(rename = "businessName")]
    pub business_name: String,
    #[serde(rename = "stellarPublicKey")]
    pub stellar_public_key: String,
    #[serde(rename = "settlementPublicKey")]
    pub settlement_public_key: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct InvoiceRequest {
    pub description: String,
    #[serde(rename = "amountUsd")]
    pub amount_usd: f64,
}

#[derive(Deserialize)]
pub struct StellarWebhookRequest {
    #[serde(rename = "publicId")]
    pub public_id: String,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: String,
    #[serde(flatten)]
    pub rest: Value,
}

impl Merchant {
    pub fn from_row(row: &Row) -> Self {
        Self {
            id: row.get("id"),
            email: row.get("email"),
            business_name: row.get("business_name"),
            stellar_public_key: row.get("stellar_public_key"),
            settlement_public_key: row.get("settlement_public_key"),
            created_at: row.get("created_at"),
        }
    }

    pub fn as_login(&self) -> LoginMerchant {
        LoginMerchant {
            id: self.id,
            email: self.email.clone(),
            business_name: self.business_name.clone(),
        }
    }
}

impl Invoice {
    pub fn from_row(row: &Row) -> Self {
        Self {
            id: row.get("id"),
            public_id: row.get("public_id"),
            merchant_id: row.get("merchant_id"),
            description: row.get("description"),
            amount_cents: row.get("amount_cents"),
            currency: row.get("currency"),
            asset_code: row.get("asset_code"),
            asset_issuer: row.get("asset_issuer"),
            destination_public_key: row.get("destination_public_key"),
            memo: row.get("memo"),
            status: row.get("status"),
            gross_amount_cents: row.get("gross_amount_cents"),
            platform_fee_cents: row.get("platform_fee_cents"),
            net_amount_cents: row.get("net_amount_cents"),
            expires_at: row.get("expires_at"),
            paid_at: row.get("paid_at"),
            settled_at: row.get("settled_at"),
            transaction_hash: row.get("transaction_hash"),
            settlement_hash: row.get("settlement_hash"),
            checkout_url: row.get("checkout_url"),
            qr_data_url: row.get("qr_data_url"),
            metadata: row.get("metadata"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        }
    }
}

#[allow(dead_code)]
impl CronRun {
    pub fn from_row(row: &Row) -> Self {
        Self {
            id: row.get("id"),
            job_type: row.get("job_type"),
            started_at: row.get("started_at"),
            finished_at: row.get("finished_at"),
            success: row.get("success"),
            metadata: row.get("metadata"),
            error_detail: row.get("error_detail"),
        }
    }
}
