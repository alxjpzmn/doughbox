use rust_decimal::Decimal;
use serde::Serialize;
use typeshare::typeshare;
use utoipa::ToSchema;

#[typeshare]
#[derive(Debug, Serialize, ToSchema)]
pub struct MonthlyInvestmentActivity {
    pub month: String,
    pub net_purchases_eur: Decimal,
    pub net_cash_added_eur: Decimal,
    pub dividends_interest_eur: Decimal,
}
