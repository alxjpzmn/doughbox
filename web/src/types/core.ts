/*
 Generated by typeshare 1.0.0
*/

export interface TaxRelevantEvent {
	date: Date;
	event_type: TaxEventType;
	currency: string;
	units: number;
	price_unit: number;
	identifier?: string;
	direction?: TradeDirection;
	applied_fx_rate?: number;
	withholding_tax_percent?: number;
}

export enum TaxEventType {
	CashInterest = "CashInterest",
	ShareInterest = "ShareInterest",
	Dividend = "Dividend",
	Trade = "Trade",
	FxConversion = "FxConversion",
	DividendAequivalent = "DividendAequivalent",
}

export enum TradeDirection {
	Buy = "Buy",
	Sell = "Sell",
}
