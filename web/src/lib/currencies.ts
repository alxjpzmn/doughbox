export const ECB_CURRENCIES = [
  { code: "EUR", name: "Euro" },
  { code: "AUD", name: "Australian dollar" },
  { code: "BRL", name: "Brazilian real" },
  { code: "CAD", name: "Canadian dollar" },
  { code: "CHF", name: "Swiss franc" },
  { code: "CNY", name: "Chinese yuan renminbi" },
  { code: "CZK", name: "Czech koruna" },
  { code: "DKK", name: "Danish krone" },
  { code: "GBP", name: "Pound sterling" },
  { code: "HKD", name: "Hong Kong dollar" },
  { code: "HUF", name: "Hungarian forint" },
  { code: "IDR", name: "Indonesian rupiah" },
  { code: "ILS", name: "Israeli shekel" },
  { code: "INR", name: "Indian rupee" },
  { code: "ISK", name: "Icelandic krona" },
  { code: "JPY", name: "Japanese yen" },
  { code: "KRW", name: "South Korean won" },
  { code: "MXN", name: "Mexican peso" },
  { code: "MYR", name: "Malaysian ringgit" },
  { code: "NOK", name: "Norwegian krone" },
  { code: "NZD", name: "New Zealand dollar" },
  { code: "PHP", name: "Philippine peso" },
  { code: "PLN", name: "Polish zloty" },
  { code: "RON", name: "Romanian leu" },
  { code: "SEK", name: "Swedish krona" },
  { code: "SGD", name: "Singapore dollar" },
  { code: "THB", name: "Thai baht" },
  { code: "TRY", name: "Turkish lira" },
  { code: "USD", name: "US dollar" },
  { code: "ZAR", name: "South African rand" },
] as const

export type EcbCurrencyCode = (typeof ECB_CURRENCIES)[number]["code"]

export const ECB_CURRENCY_CODES: readonly string[] = ECB_CURRENCIES.map((currency) => currency.code)

export const isEcbCurrency = (code?: string) => !!code && ECB_CURRENCY_CODES.includes(code)

export const currencyOptions = (current?: string) => {
  if (current && !isEcbCurrency(current)) {
    return [{ code: current, name: current }, ...ECB_CURRENCIES]
  }
  return ECB_CURRENCIES
}
