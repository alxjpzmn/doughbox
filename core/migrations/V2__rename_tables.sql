-- Migration script to rename tables to singular names

ALTER TABLE trades RENAME TO trade;
ALTER TABLE fund_reports RENAME TO fund_report_oekb;
ALTER TABLE fx_rates RENAME TO fx_rate;
ALTER TABLE stock_splits RENAME TO stock_split;
ALTER TABLE fx_conversions RENAME TO fx_conversion;
ALTER TABLE listing_changes RENAME TO listing_change;
ALTER TABLE instruments RENAME TO instrument;
ALTER TABLE dividends RENAME TO dividend;
ALTER TABLE ticker_conversions RENAME TO ticker_conversion;

