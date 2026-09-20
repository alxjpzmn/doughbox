import { yupResolver } from "@hookform/resolvers/yup"
import { format } from "date-fns"
import {
  Banknote,
  Building2,
  Coins,
  History,
  Landmark,
  LoaderCircle,
  Pencil,
  Percent,
  Plus,
  Scale,
  Trash2,
  TriangleAlert,
} from "lucide-react"
import { useEffect, useState } from "react"
import { Controller, useForm } from "react-hook-form"
import useSWR, { useSWRConfig } from "swr"
import * as yup from "yup"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from "@/components/ui/card"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Skeleton } from "@/components/ui/skeleton"
import { currencyOptions, isEcbCurrency } from "@/lib/currencies"
import { BASE_URL, fetcher, sendMutateRequest } from "@/lib/http"
import { cn, formatCurrency, formatDate, getBrowserLocale } from "@/lib/utils"
import {
  AssetBalanceSnapshotKind,
  AssetBalanceSnapshot,
  AssetClass,
  AssetDetailRecord,
  AssetListRecord,
  AssetTradeDirection,
  AssetTrade,
  AssetTransaction,
  AssetTransactionKind,
  AssetValuation,
  AssetValuationSource,
  CreateAssetBalanceSnapshotRequest,
  CreateAssetRequest,
  CreateAssetTradeRequest,
  CreateAssetTransactionRequest,
  CreateAssetValuationRequest,
  CreateLinkedInterestRequest,
  CustomAssetHolding,
  InterestRecord,
  InterestOrigin,
  LinkedAssetInterest,
  TaxTreatment,
  UpdateAssetRequest,
  WeightedCashInterestSummary,
} from "@/types/core"

type Icon = typeof Coins

const classDetails: Record<AssetClass, { label: string; description: string; icon: Icon }> = {
  [AssetClass.PhysicalGold]: {
    label: "Physical gold",
    description: "Bullion and allocated holdings",
    icon: Coins,
  },
  [AssetClass.RealEstate]: {
    label: "Real estate",
    description: "Direct property ownership",
    icon: Building2,
  },
  [AssetClass.PrivateDebt]: {
    label: "Private debt",
    description: "Loans and private credit",
    icon: Banknote,
  },
  [AssetClass.CashAccount]: {
    label: "Cash account",
    description: "Cash with an independently tracked rate",
    icon: Landmark,
  },
}

const assetDefaults: Record<AssetClass, { currency: string; unitLabel: string; rate: string }> = {
  [AssetClass.PhysicalGold]: { currency: "EUR", unitLabel: "g", rate: "" },
  [AssetClass.RealEstate]: { currency: "EUR", unitLabel: "property", rate: "" },
  [AssetClass.PrivateDebt]: { currency: "EUR", unitLabel: "EUR", rate: "" },
  [AssetClass.CashAccount]: { currency: "EUR", unitLabel: "EUR", rate: "0" },
}

const today = () => format(new Date(), "yyyy-MM-dd")
const toApiDate = (value: string) => new Date(`${value}T00:00:00`)
const toFormDate = (value: Date) => format(new Date(value), "yyyy-MM-dd")
const optionalValue = (value: string) => value.trim() || undefined
const formatNumber = (value: string | number) =>
  new Intl.NumberFormat(getBrowserLocale(), { maximumFractionDigits: 6 }).format(Number(value))
const formatDateTime = (value: Date) =>
  new Intl.DateTimeFormat(getBrowserLocale(), {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(value)

const getErrorMessage = (error: unknown) => {
  if (typeof error === "object" && error !== null && "message" in error) {
    return String(error.message)
  }
  return "The request could not be completed. Please try again."
}

const FormMessage = ({ message }: { message?: string }) =>
  message ? <p className="text-destructive text-xs leading-relaxed">{message}</p> : null

const CurrencySelect = ({
  id,
  value,
  onChange,
  invalid,
}: {
  id: string
  value: string
  onChange: (value: string) => void
  invalid?: boolean
}) => (
  <Select value={value} onValueChange={onChange}>
    <SelectTrigger id={id} aria-invalid={invalid}>
      <SelectValue />
    </SelectTrigger>
    <SelectContent>
      {currencyOptions(value).map((currency) => (
        <SelectItem key={currency.code} value={currency.code}>
          {currency.code} · {currency.name}
        </SelectItem>
      ))}
    </SelectContent>
  </Select>
)

interface AssetFormValues {
  name: string
  asset_class: AssetClass
  currency: string
  unit_label: string
  current_interest_rate_percent: string
}

const assetSchema: yup.ObjectSchema<AssetFormValues> = yup.object({
  name: yup.string().trim().required("Name is required"),
  asset_class: yup.mixed<AssetClass>().oneOf(Object.values(AssetClass)).required(),
  currency: yup
    .string()
    .required("Currency is required")
    .test("ecb", "Select a supported currency", (value) => isEcbCurrency(value) || /^[A-Z]{3}$/.test(value ?? "")),
  unit_label: yup.string().trim().required("Unit label is required"),
  current_interest_rate_percent: yup
    .string()
    .defined()
    .test("rate", "Enter a rate between 0 and 100", function (value) {
      if (this.parent.asset_class !== AssetClass.CashAccount && value === "") return true
      if (value === "") return this.createError({ message: "Current rate is required for cash accounts" })
      const number = Number(value)
      return Number.isFinite(number) && number >= 0 && number <= 100
    }),
})

interface AssetFormDialogProps {
  asset?: AssetListRecord
  open: boolean
  onOpenChange: (open: boolean) => void
  onMutated: () => Promise<unknown>
}

const AssetFormDialog = ({ asset, open, onOpenChange, onMutated }: AssetFormDialogProps) => {
  const [serverError, setServerError] = useState<string>()
  const {
    control,
    register,
    handleSubmit,
    reset,
    setValue,
    watch,
    formState: { errors, isSubmitting },
  } = useForm<AssetFormValues>({
    resolver: yupResolver(assetSchema),
    defaultValues: {
      name: "",
      asset_class: AssetClass.PhysicalGold,
      currency: "EUR",
      unit_label: "g",
      current_interest_rate_percent: "",
    },
  })

  useEffect(() => {
    if (!open) return
    const defaults = assetDefaults[asset?.asset_class ?? AssetClass.PhysicalGold]
    reset({
      name: asset?.name ?? "",
      asset_class: asset?.asset_class ?? AssetClass.PhysicalGold,
      currency: asset?.currency ?? defaults.currency,
      unit_label: asset?.unit_label ?? defaults.unitLabel,
      current_interest_rate_percent: asset?.current_interest_rate_percent ?? defaults.rate,
    })
    setServerError(undefined)
  }, [asset, open, reset])

  const assetClass = watch("asset_class")
  const currency = watch("currency")
  const usesCurrencyUnits = assetClass === AssetClass.CashAccount || assetClass === AssetClass.PrivateDebt

  useEffect(() => {
    if (usesCurrencyUnits && currency) {
      setValue("unit_label", currency, { shouldValidate: true })
    }
  }, [currency, setValue, usesCurrencyUnits])

  const submit = async (values: AssetFormValues) => {
    setServerError(undefined)
    const rate =
      values.asset_class === AssetClass.CashAccount
        ? values.current_interest_rate_percent.trim()
        : undefined

    try {
      if (asset) {
        const payload: UpdateAssetRequest = {
          name: values.name.trim(),
          currency: values.currency,
          unit_label: values.unit_label.trim(),
          current_interest_rate_percent: rate,
        }
        await sendMutateRequest(`${BASE_URL}/assets/${asset.id}`, payload, { method: "PUT" })
      } else {
        const payload: CreateAssetRequest = {
          name: values.name.trim(),
          asset_class: values.asset_class,
          currency: values.currency,
          unit_label: values.unit_label.trim(),
          current_interest_rate_percent: rate,
        }
        await sendMutateRequest(`${BASE_URL}/assets`, payload)
      }
      await onMutated()
      onOpenChange(false)
    } catch (error) {
      setServerError(getErrorMessage(error))
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{asset ? "Edit asset" : "Add a custom asset"}</DialogTitle>
          <DialogDescription>
            {asset
              ? "Update its display details and current rate. Currency cannot change after activity is recorded."
              : "Choose the ledger behavior that matches what you own."}
          </DialogDescription>
        </DialogHeader>

        <form className="space-y-4" onSubmit={handleSubmit(submit)} noValidate>
          {serverError && (
            <div role="alert" className="bg-destructive/10 text-destructive rounded-lg border border-destructive/20 p-3 text-sm">
              {serverError}
            </div>
          )}

          <div className="space-y-2">
            <Label htmlFor="asset-class">Asset class</Label>
            {asset ? (
              <div className="bg-muted/60 flex items-center gap-3 rounded-lg border p-3">
                {(() => {
                  const AssetIcon = classDetails[asset.asset_class].icon
                  return <AssetIcon className="size-4" />
                })()}
                <span className="text-sm font-medium">{classDetails[asset.asset_class].label}</span>
              </div>
            ) : (
              <Controller
                name="asset_class"
                control={control}
                render={({ field }) => (
                  <Select
                    value={field.value}
                    onValueChange={(value: AssetClass) => {
                      field.onChange(value)
                      const defaults = assetDefaults[value]
                      setValue("currency", defaults.currency, { shouldValidate: true })
                      setValue("unit_label", defaults.unitLabel, { shouldValidate: true })
                      setValue("current_interest_rate_percent", defaults.rate, { shouldValidate: true })
                    }}
                  >
                    <SelectTrigger id="asset-class">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {Object.values(AssetClass).map((value) => (
                        <SelectItem key={value} value={value}>
                          {classDetails[value].label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              />
            )}
            <p className="text-muted-foreground text-xs">{classDetails[assetClass].description}</p>
          </div>

          <div className="space-y-2">
            <Label htmlFor="asset-name">Name</Label>
            <Input id="asset-name" placeholder="e.g. Main savings" autoFocus {...register("name")} aria-invalid={!!errors.name} />
            <FormMessage message={errors.name?.message} />
          </div>

          <div className={cn("grid gap-3", !usesCurrencyUnits && "sm:grid-cols-2")}>
            <div className="space-y-2">
              <Label htmlFor="asset-currency">Currency</Label>
              <Controller
                name="currency"
                control={control}
                render={({ field }) => (
                  <CurrencySelect
                    id="asset-currency"
                    value={field.value}
                    onChange={field.onChange}
                    invalid={!!errors.currency}
                  />
                )}
              />
              <FormMessage message={errors.currency?.message} />
            </div>
            {!usesCurrencyUnits && (
              <div className="space-y-2">
                <Label htmlFor="asset-unit">Unit label</Label>
                <Input id="asset-unit" placeholder="g, property" {...register("unit_label")} aria-invalid={!!errors.unit_label} />
                <FormMessage message={errors.unit_label?.message} />
              </div>
            )}
          </div>

          {assetClass === AssetClass.CashAccount && (
            <div className="space-y-2">
              <Label htmlFor="asset-rate">Current gross annual rate (%)</Label>
              <div className="relative">
                <Input
                  id="asset-rate"
                  type="number"
                  min="0"
                  max="100"
                  step="any"
                  className="pr-9"
                  {...register("current_interest_rate_percent")}
                  aria-invalid={!!errors.current_interest_rate_percent}
                />
                <Percent className="text-muted-foreground pointer-events-none absolute right-3 top-2.5 size-4" />
              </div>
              <FormMessage message={errors.current_interest_rate_percent?.message} />
              <p className="text-muted-foreground text-xs">Zero is a valid rate. Saving a change updates this account's rate timestamp.</p>
            </div>
          )}

          <DialogFooter className="pt-2">
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={isSubmitting}>
              {isSubmitting && <LoaderCircle className="animate-spin" />}
              {asset ? "Save changes" : "Create asset"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

type ActivityKind =
  | "buy"
  | "sell"
  | "valuation"
  | "deposit"
  | "withdrawal"
  | "advance"
  | "repayment"
  | "interest"
  | "opening"
  | "reconciliation"
  | "link"

const activityOptions: Record<AssetClass, { value: ActivityKind; label: string }[]> = {
  [AssetClass.PhysicalGold]: [
    { value: "buy", label: "Buy" },
    { value: "sell", label: "Sell" },
    { value: "valuation", label: "Manual valuation" },
  ],
  [AssetClass.RealEstate]: [
    { value: "buy", label: "Buy" },
    { value: "sell", label: "Sell" },
    { value: "valuation", label: "Manual valuation" },
  ],
  [AssetClass.PrivateDebt]: [
    { value: "advance", label: "Principal advance" },
    { value: "repayment", label: "Principal repayment" },
    { value: "interest", label: "Actual interest" },
    { value: "link", label: "Link existing interest" },
  ],
  [AssetClass.CashAccount]: [
    { value: "deposit", label: "Deposit" },
    { value: "withdrawal", label: "Withdrawal" },
    { value: "interest", label: "Actual interest" },
    { value: "opening", label: "Opening balance" },
    { value: "reconciliation", label: "Reconciliation balance" },
    { value: "link", label: "Link existing interest" },
  ],
}

interface ActivityFormValues {
  action: ActivityKind
  date: string
  units: string
  price: string
  exact_eur: string
  amount: string
  balance: string
  broker: string
  fees: string
  note: string
  withholding_tax: string
  withholding_tax_currency: string
  interest_id: string
}

interface ActivityEdit {
  endpoint: string
  values: ActivityFormValues
}

const emptyActivityValues = (asset: Pick<AssetListRecord, "id" | "currency">, action: ActivityKind): ActivityFormValues => ({
  action,
  date: today(),
  units: "",
  price: "",
  exact_eur: "",
  amount: "",
  balance: "",
  broker: "",
  fees: "",
  note: "",
  withholding_tax: "",
  withholding_tax_currency: asset.currency,
  interest_id: "",
})

const optionalEur = (asset: Pick<AssetListRecord, "currency">, value: string) =>
  asset.currency === "EUR" ? "" : value

const tradeEdit = (asset: Pick<AssetListRecord, "id" | "currency">, trade: AssetTrade): ActivityEdit => ({
  endpoint: `${BASE_URL}/assets/${asset.id}/trades/${trade.id}`,
  values: {
    ...emptyActivityValues(asset, trade.direction === AssetTradeDirection.Buy ? "buy" : "sell"),
    date: toFormDate(new Date(trade.date)),
    units: trade.units,
    price: trade.price_per_unit,
    exact_eur: optionalEur(asset, trade.eur_price_per_unit),
    broker: trade.broker,
    fees: trade.fees,
    note: trade.note ?? "",
  },
})

const valuationEdit = (asset: Pick<AssetListRecord, "id" | "currency">, valuation: AssetValuation): ActivityEdit => ({
  endpoint: `${BASE_URL}/assets/${asset.id}/valuations/${valuation.id}`,
  values: {
    ...emptyActivityValues(asset, "valuation"),
    date: toFormDate(new Date(valuation.date)),
    price: valuation.price_per_unit,
    exact_eur: optionalEur(asset, valuation.eur_price_per_unit),
  },
})

const transactionEdit = (asset: Pick<AssetListRecord, "id" | "currency">, transaction: AssetTransaction): ActivityEdit => ({
  endpoint: `${BASE_URL}/assets/${asset.id}/transactions/${transaction.id}`,
  values: {
    ...emptyActivityValues(asset, transaction.kind === AssetTransactionKind.Deposit
      ? "deposit"
      : transaction.kind === AssetTransactionKind.Withdrawal
        ? "withdrawal"
        : transaction.kind === AssetTransactionKind.PrincipalAdvance
          ? "advance"
          : "repayment"),
    date: toFormDate(new Date(transaction.date)),
    amount: transaction.amount,
    exact_eur: optionalEur(asset, transaction.amount_eur),
    note: transaction.note ?? "",
  },
})

const snapshotEdit = (asset: Pick<AssetListRecord, "id" | "currency">, snapshot: AssetBalanceSnapshot): ActivityEdit => ({
  endpoint: `${BASE_URL}/assets/${asset.id}/balance-snapshots/${snapshot.id}`,
  values: {
    ...emptyActivityValues(asset, snapshot.kind === AssetBalanceSnapshotKind.Opening ? "opening" : "reconciliation"),
    date: toFormDate(new Date(snapshot.date)),
    balance: snapshot.balance,
    exact_eur: optionalEur(asset, snapshot.balance_eur),
    note: snapshot.note ?? "",
  },
})

const interestEdit = (asset: Pick<AssetListRecord, "id" | "currency">, record: LinkedAssetInterest): ActivityEdit => ({
  endpoint: `${BASE_URL}/assets/${asset.id}/interest/${record.id}`,
  values: {
    ...emptyActivityValues(asset, "interest"),
    date: toFormDate(new Date(record.date)),
    amount: record.amount,
    exact_eur: optionalEur(asset, record.amount_eur),
    broker: record.broker ?? "",
    withholding_tax: record.withholding_tax ?? "",
    withholding_tax_currency: record.withholding_tax_currency ?? asset.currency,
  },
})

interface ActivityDialogProps {
  asset?: AssetListRecord
  open: boolean
  onOpenChange: (open: boolean) => void
  interest?: InterestRecord[]
  interestError?: unknown
  interestLoading: boolean
  edit?: ActivityEdit
  onMutated: () => Promise<unknown>
}

const ActivityDialog = ({
  asset,
  open,
  onOpenChange,
  interest,
  interestError,
  interestLoading,
  edit,
  onMutated,
}: ActivityDialogProps) => {
  const [serverError, setServerError] = useState<string>()
  const {
    control,
    register,
    handleSubmit,
    reset,
    setError,
    clearErrors,
    watch,
    formState: { errors, isSubmitting },
  } = useForm<ActivityFormValues>()
  const detailRequest = useSWR<AssetDetailRecord>(
    open && asset ? `${BASE_URL}/assets/${asset.id}` : null,
    fetcher,
  )

  useEffect(() => {
    if (!asset || !open) return
    reset(edit?.values ?? {
      action: activityOptions[asset.asset_class][0].value,
      date: today(),
      units: "",
      price: "",
      exact_eur: "",
      amount: "",
      balance: "",
      broker: "",
      fees: "",
      note: "",
      withholding_tax: "",
      withholding_tax_currency: asset.currency,
      interest_id: "",
    })
    setServerError(undefined)
  }, [asset, edit, open, reset])

  if (!asset) return null

  const action = watch("action")
  const isTrade = action === "buy" || action === "sell"
  const isValuation = action === "valuation"
  const isTransaction = ["deposit", "withdrawal", "advance", "repayment"].includes(action)
  const isInterest = action === "interest"
  const isSnapshot = action === "opening" || action === "reconciliation"
  const isLink = action === "link"
  const matchingInterest = (interest ?? []).filter(
    (record) =>
      !record.asset_id
      && record.currency === asset.currency
      && record.principal === (asset.asset_class === AssetClass.CashAccount ? "Cash" : "PrivateDebt"),
  )
  const hasOpeningSnapshot = detailRequest.data?.activity.asset_class === AssetClass.CashAccount
    && detailRequest.data.activity.activity.balance_snapshots.some(
      (snapshot) => snapshot.kind === AssetBalanceSnapshotKind.Opening,
    )
  const availableActivityOptions = activityOptions[asset.asset_class].filter(
    (option) => option.value !== "opening" || !hasOpeningSnapshot || edit?.values.action === "opening",
  )

  const validateNumber = (
    field: "units" | "price" | "amount" | "balance" | "exact_eur" | "fees" | "withholding_tax",
    value: string,
    label: string,
    allowZero = false,
    optional = false,
  ) => {
    if (optional && value.trim() === "") return true
    const number = Number(value)
    if (!Number.isFinite(number) || (allowZero ? number < 0 : number <= 0)) {
      setError(field, { message: `${label} must be ${allowZero ? "zero or greater" : "greater than zero"}` })
      return false
    }
    return true
  }

  const submit = async (values: ActivityFormValues) => {
    clearErrors()
    setServerError(undefined)

    if (values.action === "link") {
      if (!values.interest_id) {
        setError("interest_id", { message: "Choose an interest event" })
        return
      }
      try {
        await sendMutateRequest(`${BASE_URL}/assets/${asset.id}/interest/${values.interest_id}`)
        await onMutated()
        onOpenChange(false)
      } catch (error) {
        setServerError(getErrorMessage(error))
      }
      return
    }

    if (!values.date) {
      setError("date", { message: "Date is required" })
      return
    }

    let valid = true
    if (isTrade) {
      valid = validateNumber("units", values.units, "Units") && valid
      valid = validateNumber("price", values.price, "Price", true) && valid
      if (!values.broker.trim()) {
        setError("broker", { message: "Broker or counterparty is required" })
        valid = false
      }
      valid = validateNumber("fees", values.fees, "Fees", true, true) && valid
    }
    if (isValuation) valid = validateNumber("price", values.price, "Price", true) && valid
    if (isTransaction || isInterest) valid = validateNumber("amount", values.amount, "Amount") && valid
    if (isSnapshot) valid = validateNumber("balance", values.balance, "Balance", true) && valid
    if (asset.currency !== "EUR" && values.exact_eur.trim() !== "") {
      const nativeValue = isTrade || isValuation
        ? Number(values.price)
        : isSnapshot
          ? Number(values.balance)
          : Number(values.amount)
      valid = validateNumber("exact_eur", values.exact_eur, "EUR equivalent", nativeValue === 0) && valid
    }
    if (isInterest) {
      valid = validateNumber("withholding_tax", values.withholding_tax, "Withholding tax", true, true) && valid
      if (Number(values.withholding_tax) > 0 && !values.withholding_tax_currency) {
        setError("withholding_tax_currency", { message: "Withholding tax currency is required" })
        valid = false
      } else if (
        Number(values.withholding_tax) > 0 &&
        !isEcbCurrency(values.withholding_tax_currency)
      ) {
        setError("withholding_tax_currency", { message: "Select a supported currency" })
        valid = false
      }
    }
    if (!valid) return

    try {
      const date = toApiDate(values.date)
      if (isTrade) {
        const payload: CreateAssetTradeRequest = {
          date,
          units: values.units,
          price_per_unit: values.price,
          eur_price_per_unit: optionalValue(values.exact_eur),
          direction: values.action === "buy" ? AssetTradeDirection.Buy : AssetTradeDirection.Sell,
          currency: asset.currency,
          broker: values.broker.trim(),
          fees: optionalValue(values.fees),
          note: optionalValue(values.note),
        }
        await sendMutateRequest(edit?.endpoint ?? `${BASE_URL}/assets/${asset.id}/trades`, payload, edit ? { method: "PUT" } : undefined)
      } else if (isValuation) {
        const payload: CreateAssetValuationRequest = {
          date,
          price_per_unit: values.price,
          eur_price_per_unit: optionalValue(values.exact_eur),
          currency: asset.currency,
        }
        await sendMutateRequest(edit?.endpoint ?? `${BASE_URL}/assets/${asset.id}/valuations`, payload, edit ? { method: "PUT" } : undefined)
      } else if (isTransaction) {
        const kind: Record<"deposit" | "withdrawal" | "advance" | "repayment", AssetTransactionKind> = {
          deposit: AssetTransactionKind.Deposit,
          withdrawal: AssetTransactionKind.Withdrawal,
          advance: AssetTransactionKind.PrincipalAdvance,
          repayment: AssetTransactionKind.PrincipalRepayment,
        }
        const payload: CreateAssetTransactionRequest = {
          date,
          kind: kind[values.action as keyof typeof kind],
          amount: values.amount,
          amount_eur: optionalValue(values.exact_eur),
          currency: asset.currency,
          note: optionalValue(values.note),
        }
        await sendMutateRequest(edit?.endpoint ?? `${BASE_URL}/assets/${asset.id}/transactions`, payload, edit ? { method: "PUT" } : undefined)
      } else if (isSnapshot) {
        const payload: CreateAssetBalanceSnapshotRequest = {
          date,
          kind:
            values.action === "opening"
              ? AssetBalanceSnapshotKind.Opening
              : AssetBalanceSnapshotKind.Reconciliation,
          balance: values.balance,
          balance_eur: optionalValue(values.exact_eur),
          note: optionalValue(values.note),
        }
        await sendMutateRequest(edit?.endpoint ?? `${BASE_URL}/assets/${asset.id}/balance-snapshots`, payload, edit ? { method: "PUT" } : undefined)
      } else if (isInterest) {
        const payload: CreateLinkedInterestRequest = {
          date,
          amount: values.amount,
          amount_eur: optionalValue(values.exact_eur),
          currency: asset.currency,
          broker: optionalValue(values.broker),
          withholding_tax: optionalValue(values.withholding_tax),
          withholding_tax_currency:
            Number(values.withholding_tax) > 0
              ? optionalValue(values.withholding_tax_currency)
              : undefined,
        }
        await sendMutateRequest(edit?.endpoint ?? `${BASE_URL}/assets/${asset.id}/interest`, payload, edit ? { method: "PUT" } : undefined)
      }
      await onMutated()
      onOpenChange(false)
    } catch (error) {
      setServerError(getErrorMessage(error))
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{edit ? "Edit activity" : "Record activity"}</DialogTitle>
          <DialogDescription>
            {asset.name} · {classDetails[asset.asset_class].label}
          </DialogDescription>
        </DialogHeader>

        <form className="space-y-4" onSubmit={handleSubmit(submit)} noValidate>
          {serverError && (
            <div role="alert" className="bg-destructive/10 text-destructive rounded-lg border border-destructive/20 p-3 text-sm">
              {serverError}
            </div>
          )}

          <div className="space-y-2">
            <Label htmlFor="activity-kind">Activity</Label>
            {edit ? (
              <div className="bg-muted/60 rounded-lg border p-3 text-sm font-medium">
                {availableActivityOptions.find((option) => option.value === action)?.label}
              </div>
            ) : (
              <Controller
                name="action"
                control={control}
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
                    <SelectTrigger id="activity-kind">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {availableActivityOptions.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              />
            )}
          </div>

          {isInterest && (
            <div className="text-muted-foreground rounded-lg border bg-muted/40 p-3 text-sm leading-relaxed">
              Manually added interest is excluded from automated taxation. It still contributes to return attribution.
            </div>
          )}

          {isSnapshot && (
            <div className="text-muted-foreground rounded-lg border bg-muted/40 p-3 text-sm leading-relaxed">
              Balance updates are capital-neutral. Record interest separately so returns are attributed correctly.
            </div>
          )}

          {isLink ? (
            <div className="space-y-4">
              <div className="text-muted-foreground rounded-lg border bg-muted/40 p-3 text-sm leading-relaxed">
                Linking only assigns this event to the asset. Its existing tax behavior remains unchanged.
              </div>
              {interestLoading ? (
                <Skeleton className="h-9 w-full" />
              ) : interestError ? (
                <p className="text-destructive text-sm">{getErrorMessage(interestError)}</p>
              ) : matchingInterest.length === 0 ? (
                <div className="bg-muted/50 rounded-lg border border-dashed p-5 text-center">
                  <p className="text-sm font-medium">No matching unlinked interest</p>
                  <p className="text-muted-foreground mt-1 text-xs">Only unlinked {asset.currency} {asset.asset_class === AssetClass.CashAccount ? "cash" : "private debt"} events can be assigned here.</p>
                </div>
              ) : (
                <div className="space-y-2">
                  <Label htmlFor="interest-event">Existing interest event</Label>
                  <Controller
                    name="interest_id"
                    control={control}
                    render={({ field }) => (
                      <Select value={field.value} onValueChange={field.onChange}>
                        <SelectTrigger id="interest-event" aria-invalid={!!errors.interest_id}>
                          <SelectValue placeholder="Choose an event" />
                        </SelectTrigger>
                        <SelectContent>
                          {matchingInterest.map((record) => (
                            <SelectItem key={record.id} value={record.id}>
                              {formatDate(new Date(record.date))} · {formatCurrency(Number(record.amount), record.currency)} · {record.broker || "Unknown broker"} · {record.principal || "Unknown"} · {record.origin} · {record.tax_treatment}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    )}
                  />
                  <FormMessage message={errors.interest_id?.message} />
                </div>
              )}
            </div>
          ) : (
            <>
              <div className="space-y-2">
                <Label htmlFor="activity-date">Date</Label>
                <Input id="activity-date" type="date" {...register("date")} aria-invalid={!!errors.date} />
                <FormMessage message={errors.date?.message} />
              </div>

              {isTrade && (
                <div className="grid gap-3 sm:grid-cols-2">
                  <div className="space-y-2">
                    <Label htmlFor="activity-units">Units ({asset.unit_label})</Label>
                    <Input id="activity-units" type="number" min="0" step="any" {...register("units")} aria-invalid={!!errors.units} />
                    <FormMessage message={errors.units?.message} />
                  </div>
                  <div className="space-y-2">
                    <Label htmlFor="activity-price">Price per unit ({asset.currency})</Label>
                    <Input id="activity-price" type="number" min="0" step="any" {...register("price")} aria-invalid={!!errors.price} />
                    <FormMessage message={errors.price?.message} />
                  </div>
                </div>
              )}

              {isValuation && (
                <div className="space-y-2">
                  <Label htmlFor="activity-price">Current price per {asset.unit_label} ({asset.currency})</Label>
                  <Input id="activity-price" type="number" min="0" step="any" {...register("price")} aria-invalid={!!errors.price} />
                  <FormMessage message={errors.price?.message} />
                </div>
              )}

              {(isTransaction || isInterest) && (
                <div className="space-y-2">
                    <Label htmlFor="activity-amount">{isInterest && asset.asset_class === AssetClass.CashAccount ? "Net amount credited" : "Amount"} ({asset.currency})</Label>
                  <Input id="activity-amount" type="number" min="0" step="any" {...register("amount")} aria-invalid={!!errors.amount} />
                  <FormMessage message={errors.amount?.message} />
                </div>
              )}

              {isSnapshot && (
                <div className="space-y-2">
                  <Label htmlFor="activity-balance">Absolute balance ({asset.currency})</Label>
                  <Input id="activity-balance" type="number" min="0" step="any" {...register("balance")} aria-invalid={!!errors.balance} />
                  <FormMessage message={errors.balance?.message} />
                </div>
              )}

              {asset.currency !== "EUR" && (
                <div className="space-y-2">
                  <Label htmlFor="activity-eur">
                    {isTrade || isValuation ? "Exact EUR price per unit" : isSnapshot ? "Exact EUR balance" : "Exact EUR equivalent"} (optional)
                  </Label>
                  <Input id="activity-eur" type="number" min="0" step="any" {...register("exact_eur")} aria-invalid={!!errors.exact_eur} />
                  <FormMessage message={errors.exact_eur?.message} />
                  <p className="text-muted-foreground text-xs">Leave blank to use the available market conversion for this date.</p>
                </div>
              )}

              {isTrade && (
                <div className="grid gap-3 sm:grid-cols-2">
                  <div className="space-y-2">
                    <Label htmlFor="activity-broker">Broker / counterparty</Label>
                    <Input id="activity-broker" {...register("broker")} aria-invalid={!!errors.broker} />
                    <FormMessage message={errors.broker?.message} />
                  </div>
                  <div className="space-y-2">
                    <Label htmlFor="activity-fees">Fees ({asset.currency})</Label>
                    <Input id="activity-fees" type="number" min="0" step="any" placeholder="0" {...register("fees")} aria-invalid={!!errors.fees} />
                    <FormMessage message={errors.fees?.message} />
                  </div>
                </div>
              )}

              {isInterest && (
                <>
                  <div className="space-y-2">
                    <Label htmlFor="activity-broker">Payer / broker (optional)</Label>
                    <Input id="activity-broker" {...register("broker")} />
                  </div>
                  <div className="grid gap-3 sm:grid-cols-2">
                    <div className="space-y-2">
                      <Label htmlFor="withholding-tax">Withholding tax</Label>
                      <Input id="withholding-tax" type="number" min="0" step="any" placeholder="0" {...register("withholding_tax")} aria-invalid={!!errors.withholding_tax} />
                      <FormMessage message={errors.withholding_tax?.message} />
                    </div>
                    <div className="space-y-2">
                      <Label htmlFor="withholding-currency">Tax currency</Label>
                      <Controller
                        name="withholding_tax_currency"
                        control={control}
                        render={({ field }) => (
                          <CurrencySelect
                            id="withholding-currency"
                            value={field.value}
                            onChange={field.onChange}
                            invalid={!!errors.withholding_tax_currency}
                          />
                        )}
                      />
                      <FormMessage message={errors.withholding_tax_currency?.message} />
                    </div>
                  </div>
                </>
              )}

              {(isTrade || isTransaction || isSnapshot) && (
                <div className="space-y-2">
                  <Label htmlFor="activity-note">Note (optional)</Label>
                  <Input id="activity-note" placeholder="Add context" {...register("note")} />
                </div>
              )}
            </>
          )}

          <DialogFooter className="pt-2">
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button
              type="submit"
              disabled={isSubmitting || (isLink && (interestLoading || matchingInterest.length === 0))}
            >
              {isSubmitting && <LoaderCircle className="animate-spin" />}
              {isLink ? "Link interest" : edit ? "Save changes" : "Record activity"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

interface HistoryItem {
  id: string
  date: Date
  title: string
  amount: string
  detail: string
  taxTreatment?: TaxTreatment
  removeEndpoint: string
  removeLabel: string
  edit?: ActivityEdit
}

const getHistoryItems = (detail: AssetDetailRecord): HistoryItem[] => {
  const items: HistoryItem[] = []
  const taggedActivity = detail.activity

  if (taggedActivity.asset_class === "PhysicalGold" || taggedActivity.asset_class === "RealEstate") {
    taggedActivity.activity.trades.forEach((trade) => {
      items.push({
        id: trade.id,
        date: new Date(trade.date),
        title: trade.direction,
        amount: `${formatNumber(trade.units)} ${detail.asset.unit_label}`,
        detail: [
          `${formatCurrency(Number(trade.price_per_unit), trade.currency)} per ${detail.asset.unit_label}`,
          `${formatCurrency(Number(trade.eur_price_per_unit), "EUR")} EUR`,
          trade.broker,
          Number(trade.fees) > 0 ? `fees ${formatCurrency(Number(trade.fees), trade.currency)}` : undefined,
          trade.note,
        ].filter(Boolean).join(" · "),
        removeEndpoint: `${BASE_URL}/assets/${detail.asset.id}/trades/${trade.id}`,
        removeLabel: "Delete",
        edit: tradeEdit(detail.asset, trade),
      })
    })
    taggedActivity.activity.valuations
      .filter((valuation) => valuation.source === AssetValuationSource.Manual)
      .forEach((valuation) => {
        items.push({
          id: valuation.id,
          date: new Date(valuation.date),
          title: "Manual valuation",
          amount: formatCurrency(Number(valuation.price_per_unit), valuation.currency),
          detail: `per ${detail.asset.unit_label} · ${formatCurrency(Number(valuation.eur_price_per_unit), "EUR")} EUR`,
          removeEndpoint: `${BASE_URL}/assets/${detail.asset.id}/valuations/${valuation.id}`,
          removeLabel: "Delete",
          edit: valuationEdit(detail.asset, valuation),
        })
      })
  } else {
    taggedActivity.activity.transactions.forEach((transaction) => {
      items.push({
        id: transaction.id,
        date: new Date(transaction.date),
        title: transaction.kind.replace(/([a-z])([A-Z])/g, "$1 $2"),
        amount: formatCurrency(Number(transaction.amount), transaction.currency),
        detail: [
          `${formatCurrency(Number(transaction.amount_eur), "EUR")} equivalent`,
          transaction.note,
        ].filter(Boolean).join(" · "),
        removeEndpoint: `${BASE_URL}/assets/${detail.asset.id}/transactions/${transaction.id}`,
        removeLabel: "Delete",
        edit: transactionEdit(detail.asset, transaction),
      })
    })
    taggedActivity.activity.interest.forEach((record) => {
      items.push({
        id: record.id,
        date: new Date(record.date),
        title: "Interest",
        amount: formatCurrency(Number(record.amount), record.currency),
        detail: [
          `${formatCurrency(Number(record.amount_eur), "EUR")} equivalent`,
          record.broker,
          record.principal,
          record.withholding_tax && Number(record.withholding_tax) > 0
            ? `WHT ${formatCurrency(Number(record.withholding_tax), record.withholding_tax_currency || record.currency)}`
            : undefined,
        ].filter(Boolean).join(" · "),
        taxTreatment: record.tax_treatment,
        removeEndpoint: `${BASE_URL}/assets/${detail.asset.id}/interest/${record.id}`,
        removeLabel: record.origin === InterestOrigin.Manual ? "Delete" : "Unlink",
        edit: record.origin === InterestOrigin.Manual ? interestEdit(detail.asset, record) : undefined,
      })
    })
    if (taggedActivity.asset_class === "CashAccount") {
      taggedActivity.activity.balance_snapshots.forEach((snapshot) => {
        items.push({
          id: snapshot.id,
          date: new Date(snapshot.date),
          title: `${snapshot.kind} balance`,
          amount: formatCurrency(Number(snapshot.balance), detail.asset.currency),
          detail: [
            `${formatCurrency(Number(snapshot.balance_eur), "EUR")} equivalent · capital-neutral`,
            snapshot.note,
          ].filter(Boolean).join(" · "),
          removeEndpoint: `${BASE_URL}/assets/${detail.asset.id}/balance-snapshots/${snapshot.id}`,
          removeLabel: "Delete",
          edit: snapshotEdit(detail.asset, snapshot),
        })
      })
    }
  }

  return items.sort((a, b) => b.date.getTime() - a.date.getTime())
}

interface HistoryDialogProps {
  asset?: AssetListRecord
  open: boolean
  onOpenChange: (open: boolean) => void
  onMutated: () => Promise<unknown>
  onEdit?: (item: HistoryItem) => void
  readOnly?: boolean
}

const HistoryDialog = ({ asset, open, onOpenChange, onMutated, onEdit, readOnly }: HistoryDialogProps) => {
  const [removing, setRemoving] = useState<string>()
  const [serverError, setServerError] = useState<string>()
  const { data, error, isLoading } = useSWR<AssetDetailRecord>(
    open && asset ? `${BASE_URL}/assets/${asset.id}` : null,
    fetcher,
  )
  const items = data ? getHistoryItems(data) : []

  const remove = async (item: HistoryItem) => {
    if (!asset) return
    if (!window.confirm(`${item.removeLabel} this ${item.title.toLowerCase()} entry?`)) return
    setRemoving(item.id)
    setServerError(undefined)
    try {
      await sendMutateRequest(
        item.removeEndpoint,
        undefined,
        { method: "DELETE" },
      )
      await onMutated()
    } catch (removeError) {
      setServerError(getErrorMessage(removeError))
    } finally {
      setRemoving(undefined)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Activity history</DialogTitle>
          <DialogDescription>{asset?.name}</DialogDescription>
        </DialogHeader>

        {serverError && (
          <div role="alert" className="bg-destructive/10 text-destructive rounded-lg border border-destructive/20 p-3 text-sm">
            {serverError}
          </div>
        )}

        {isLoading ? (
          <div className="space-y-3">
            {Array.from({ length: 4 }).map((_, index) => (
              <Skeleton key={index} className="h-16 w-full" />
            ))}
          </div>
        ) : error ? (
          <div role="alert" className="bg-destructive/10 text-destructive rounded-lg border border-destructive/20 p-3 text-sm">
            {getErrorMessage(error)}
          </div>
        ) : items.length === 0 ? (
          <div className="bg-muted/30 rounded-xl border border-dashed px-4 py-10 text-center">
            <History className="text-muted-foreground mx-auto mb-3 size-6" />
            <p className="font-medium">No activity yet</p>
            <p className="text-muted-foreground mt-1 text-sm">New entries will appear here.</p>
          </div>
        ) : (
          <div className="divide-y rounded-xl border">
            {items.map((item) => (
              <div key={`${item.title}-${item.id}`} className="flex items-center justify-between gap-3 p-3">
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <p className="text-sm font-medium">{item.title}</p>
                    {item.taxTreatment === TaxTreatment.Excluded && (
                      <Badge variant="outline">Tax excluded</Badge>
                    )}
                  </div>
                  <p className="mt-1 truncate text-sm font-semibold">{item.amount}</p>
                  <p className="text-muted-foreground truncate text-xs">{formatDate(item.date)} · {item.detail}</p>
                </div>
                {!readOnly && (
                  <div className="flex shrink-0 items-center gap-1">
                    {item.edit && onEdit && (
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        disabled={!!removing}
                        onClick={() => onEdit(item)}
                      >
                        Edit
                      </Button>
                    )}
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className={item.removeLabel === "Delete" ? "text-destructive hover:text-destructive" : undefined}
                      disabled={!!removing}
                      onClick={() => remove(item)}
                    >
                      {removing === item.id ? <LoaderCircle className="animate-spin" /> : item.removeLabel}
                    </Button>
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </DialogContent>
    </Dialog>
  )
}

const CashSummary = ({ data, isLoading }: { data?: WeightedCashInterestSummary; isLoading: boolean }) => {
  if (isLoading) {
    return (
      <Card className="mb-6 overflow-hidden">
        <CardContent className="space-y-4 p-5 sm:p-6">
          <Skeleton className="h-5 w-36" />
          <Skeleton className="h-10 w-56" />
          <Skeleton className="h-16 w-full" />
        </CardContent>
      </Card>
    )
  }
  if (!data) return null

  const accounts = data?.accounts ?? []
  return (
    <Card className="mb-6">
      <CardHeader>
        <CardTitle>Cash yield</CardTitle>
      </CardHeader>
      <CardContent>
        <div className="grid grid-cols-2 gap-4 sm:grid-cols-3">
          <div className="col-span-2 sm:col-span-1">
            <p className="text-muted-foreground text-xs">Total eligible EUR balance</p>
            <p className="mt-1 text-2xl font-bold tracking-tight">{formatCurrency(Number(data?.eligible_balance_eur ?? 0))}</p>
          </div>
          <div>
            <p className="text-muted-foreground text-xs">Volume-weighted gross rate</p>
            <p className="mt-1 text-xl font-bold">
              {data?.average_interest_rate_percent == null
                ? "Not available"
                : `${formatNumber(data.average_interest_rate_percent)}%`}
            </p>
          </div>
          <div>
            <p className="text-muted-foreground text-xs">Accounts</p>
            <p className="mt-1 text-xl font-bold">{accounts.length}</p>
          </div>
        </div>
      </CardContent>

      {data && data.unconverted_count > 0 && (
        <div className="text-muted-foreground flex gap-2 border-t px-6 py-3 text-sm">
          <TriangleAlert className="mt-0.5 size-4 shrink-0" />
          <p>
            {data.unconverted_count} {data.unconverted_count === 1 ? "account is" : "accounts are"} excluded from the weighted rate because EUR conversion is unavailable.
          </p>
        </div>
      )}

      {accounts.length > 0 && (
        <div className="grid divide-y sm:grid-cols-2 sm:divide-x sm:divide-y-0">
          {accounts.map((account) => (
            <div key={account.asset_id} className="p-4 first:pl-5 sm:px-6">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium">{account.name}</p>
                  <p className="text-muted-foreground mt-0.5 text-xs">
                    {formatCurrency(Number(account.current_balance), account.currency)}
                    {account.current_balance_eur == null && account.currency !== "EUR" ? " · conversion unavailable" : ""}
                  </p>
                </div>
                <p className="shrink-0 text-sm font-bold">{formatNumber(account.current_interest_rate_percent)}%</p>
              </div>
              <p className="text-muted-foreground mt-2 text-[11px]">
                Rate updated {account.interest_rate_updated_at ? formatDateTime(new Date(account.interest_rate_updated_at)) : "not yet"}
              </p>
            </div>
          ))}
        </div>
      )}
    </Card>
  )
}

interface AssetCardProps {
  asset: AssetListRecord
  holding?: CustomAssetHolding
  cashSummary?: WeightedCashInterestSummary
  onEdit: () => void
  onActivity: () => void
  onHistory: () => void
  onArchive: () => void
}

const AssetCard = ({ asset, holding, cashSummary, onEdit, onActivity, onHistory, onArchive }: AssetCardProps) => {
  const details = classDetails[asset.asset_class]
  const AssetIcon = details.icon
  const cashAccount = cashSummary?.accounts.find((account) => account.asset_id === asset.id)
  const isUnavailable = !holding
  const isUnpriced = !!holding && holding.current_value_eur == null

  return (
    <Card className="flex h-full flex-col">
      <CardHeader>
        <div className="flex items-start justify-between gap-3">
          <div className="flex min-w-0 items-center gap-3">
            <div className="bg-muted rounded-xl p-2.5">
              <AssetIcon className="size-5" />
            </div>
            <div className="min-w-0">
              <CardTitle className="truncate text-base">{asset.name}</CardTitle>
              <p className="text-muted-foreground mt-1 text-xs">{details.label} · {asset.currency}</p>
            </div>
          </div>
          {asset.tax_treatment === TaxTreatment.Excluded && (
            <Badge variant="outline">Tax excluded</Badge>
          )}
        </div>
      </CardHeader>
      <CardContent className="flex-1 p-5 pt-2">
        <div className="grid grid-cols-2 gap-4 rounded-lg bg-muted/40 p-3">
          <div>
            <p className="text-muted-foreground text-[11px] uppercase tracking-wide">Native amount</p>
            <p className="mt-1 truncate text-sm font-semibold">{isUnavailable ? "Unavailable" : `${formatNumber(holding.units_or_balance)} ${asset.unit_label}`}</p>
          </div>
          <div className="text-right">
            <p className="text-muted-foreground text-[11px] uppercase tracking-wide">Current value</p>
            {isUnavailable ? (
              <p className="mt-1 text-sm font-semibold text-muted-foreground">Unavailable</p>
            ) : isUnpriced ? (
              <div className="text-muted-foreground mt-1 flex items-center justify-end gap-1.5 text-sm font-semibold">
                <TriangleAlert className="size-3.5" /> Unpriced
              </div>
            ) : (
              <p className="mt-1 truncate text-sm font-semibold">{formatCurrency(Number(holding?.current_value_eur))}</p>
            )}
          </div>
        </div>

        <div className="text-muted-foreground mt-3 flex min-h-4 items-center justify-between gap-3 text-xs">
          {holding?.valuation_date ? <span>{asset.asset_class === AssetClass.CashAccount ? "Last snapshot" : "Valued"} {formatDate(new Date(holding.valuation_date))}</span> : <span />}
          {cashAccount && (
            <span className="font-medium text-foreground">
              {formatNumber(cashAccount.current_interest_rate_percent)}% gross
            </span>
          )}
        </div>
        {cashAccount && (
          <p className="text-muted-foreground mt-1 text-right text-[11px]">
            Rate updated {cashAccount.interest_rate_updated_at ? formatDateTime(new Date(cashAccount.interest_rate_updated_at)) : "not yet"}
          </p>
        )}

      </CardContent>
      <CardFooter className="grid grid-cols-2 gap-2">
        <Button type="button" onClick={onActivity}>
          <Plus /> Activity
        </Button>
        <Button type="button" variant="outline" onClick={onHistory}>
          <History /> History
        </Button>
        <Button type="button" variant="ghost" size="sm" onClick={onEdit}>
          <Pencil /> Edit
        </Button>
        <Button type="button" variant="ghost" size="sm" onClick={onArchive}>
          <Trash2 /> Archive
        </Button>
      </CardFooter>
    </Card>
  )
}

const Assets = () => {
  const [assetFormOpen, setAssetFormOpen] = useState(false)
  const [editingAsset, setEditingAsset] = useState<AssetListRecord>()
  const [activityAsset, setActivityAsset] = useState<AssetListRecord>()
  const [activityEdit, setActivityEdit] = useState<ActivityEdit>()
  const [historyAsset, setHistoryAsset] = useState<AssetListRecord>()
  const [archiveAsset, setArchiveAsset] = useState<AssetListRecord>()
  const [archiveError, setArchiveError] = useState<string>()
  const [isArchiving, setIsArchiving] = useState(false)
  const [restoringAsset, setRestoringAsset] = useState<string>()
  const [restoreError, setRestoreError] = useState<string>()
  const { mutate: mutateCache } = useSWRConfig()

  const assetsRequest = useSWR<AssetListRecord[]>(`${BASE_URL}/assets?include_archived=true`, fetcher)
  const holdingsRequest = useSWR<CustomAssetHolding[]>(`${BASE_URL}/assets/holdings`, fetcher)
  const cashRequest = useSWR<WeightedCashInterestSummary>(`${BASE_URL}/cash-accounts/summary`, fetcher)
  const interestRequest = useSWR<InterestRecord[]>(`${BASE_URL}/interest`, fetcher)

  const revalidateAssets = () =>
    mutateCache((key) => {
      if (typeof key !== "string") return false
      const resources = [
        `${BASE_URL}/assets`,
        `${BASE_URL}/assets/holdings`,
        `${BASE_URL}/cash-accounts/summary`,
        `${BASE_URL}/portfolio`,
        `${BASE_URL}/timeline`,
        `${BASE_URL}/interest`,
        `${BASE_URL}/taxation/transactions`,
      ]
      return resources.some(
        (resource) =>
          key === resource ||
          key.startsWith(`${resource}?`) ||
          (resource === `${BASE_URL}/assets` && key.startsWith(`${resource}/`)),
      )
    })

  const archive = async () => {
    if (!archiveAsset) return
    setIsArchiving(true)
    setArchiveError(undefined)
    try {
      await sendMutateRequest(`${BASE_URL}/assets/${archiveAsset.id}`, undefined, { method: "DELETE" })
      await revalidateAssets()
      setArchiveAsset(undefined)
    } catch (error) {
      setArchiveError(getErrorMessage(error))
    } finally {
      setIsArchiving(false)
    }
  }

  const holdingByAsset = new Map(
    (holdingsRequest.data ?? []).map((holding) => [holding.asset_id, holding]),
  )
  const activeAssets = (assetsRequest.data ?? []).filter((asset) => !asset.archived_at)
  const archivedAssets = (assetsRequest.data ?? []).filter((asset) => !!asset.archived_at)
  const loading = assetsRequest.isLoading || holdingsRequest.isLoading
  const pageError = assetsRequest.error || holdingsRequest.error || cashRequest.error

  const restore = async (asset: AssetListRecord) => {
    setRestoringAsset(asset.id)
    setRestoreError(undefined)
    try {
      await sendMutateRequest(`${BASE_URL}/assets/${asset.id}/restore`)
      await revalidateAssets()
    } catch (error) {
      setRestoreError(getErrorMessage(error))
    } finally {
      setRestoringAsset(undefined)
    }
  }

  return (
    <div className="pb-4">
      <div className="mb-6 flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-bold tracking-tight">Assets</h1>
          <p className="text-muted-foreground mt-1 max-w-lg text-sm">
            Track property, bullion, private credit, and interest-bearing cash with explicit valuations and cash flows.
          </p>
        </div>
        <Button
          type="button"
          className="shrink-0"
          onClick={() => {
            setEditingAsset(undefined)
            setAssetFormOpen(true)
          }}
        >
          <Plus /> <span className="hidden sm:inline">Add asset</span><span className="sm:hidden">Add</span>
        </Button>
      </div>

      <CashSummary data={cashRequest.data} isLoading={cashRequest.isLoading} />

      {pageError && (
        <div role="alert" className="bg-destructive/10 text-destructive mb-6 rounded-xl border border-destructive/20 p-4 text-sm">
          <p className="font-medium">Assets could not be loaded</p>
          <p className="mt-1">{getErrorMessage(pageError)}</p>
          <Button type="button" variant="outline" size="sm" className="mt-3" onClick={() => revalidateAssets()}>
            Try again
          </Button>
        </div>
      )}

      <div className="mb-3 flex items-end justify-between">
        <div>
          <h2 className="text-lg font-semibold">Your assets</h2>
          <p className="text-muted-foreground text-xs">Native balances and their current EUR value</p>
        </div>
        {!loading && <p className="text-muted-foreground text-xs">{activeAssets.length} active</p>}
      </div>

      {loading ? (
        <div className="grid gap-4 sm:grid-cols-2">
          {Array.from({ length: 4 }).map((_, index) => (
            <Card key={index}>
              <CardContent className="space-y-4 p-5">
                <div className="flex items-center gap-3">
                  <Skeleton className="size-10 rounded-xl" />
                  <div className="space-y-2">
                    <Skeleton className="h-4 w-32" />
                    <Skeleton className="h-3 w-20" />
                  </div>
                </div>
                <Skeleton className="h-16 w-full" />
                <Skeleton className="h-20 w-full" />
              </CardContent>
            </Card>
          ))}
        </div>
      ) : activeAssets.length === 0 ? (
        <Card className="border-dashed shadow-none">
          <CardContent className="px-5 py-12 text-center">
            <div className="bg-muted mx-auto mb-4 flex size-12 items-center justify-center rounded-2xl">
              <Scale className="text-muted-foreground size-5" />
            </div>
            <h3 className="font-semibold">Build your custom asset ledger</h3>
            <p className="text-muted-foreground mx-auto mt-2 max-w-sm text-sm">
              Add an asset, then record its opening position, transactions, or valuation.
            </p>
            <Button type="button" className="mt-5" onClick={() => {
              setEditingAsset(undefined)
              setAssetFormOpen(true)
            }}>
              <Plus /> Add your first asset
            </Button>
          </CardContent>
        </Card>
      ) : (
        <div className="grid gap-4 sm:grid-cols-2">
          {activeAssets.map((asset) => (
            <AssetCard
              key={asset.id}
              asset={asset}
              holding={holdingByAsset.get(asset.id)}
              cashSummary={cashRequest.data}
              onEdit={() => {
                setEditingAsset(asset)
                setAssetFormOpen(true)
              }}
              onActivity={() => {
                setActivityEdit(undefined)
                setActivityAsset(asset)
              }}
              onHistory={() => setHistoryAsset(asset)}
              onArchive={() => {
                setArchiveError(undefined)
                setArchiveAsset(asset)
              }}
            />
          ))}
        </div>
      )}

      {archivedAssets.length > 0 && (
        <Card className="mt-6 border-dashed shadow-none">
          <CardHeader className="pb-3">
            <CardTitle className="text-sm">Archived assets</CardTitle>
          </CardHeader>
          <CardContent className="space-y-2">
            {restoreError && <p role="alert" className="text-destructive text-sm">{restoreError}</p>}
            {archivedAssets.map((asset) => (
              <div key={asset.id} className="flex items-center justify-between gap-3 rounded-lg bg-muted/40 px-3 py-2">
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium">{asset.name}</p>
                  <p className="text-muted-foreground text-xs">{classDetails[asset.asset_class].label}</p>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <Button type="button" variant="ghost" size="sm" onClick={() => setHistoryAsset(asset)}>
                    History
                  </Button>
                  <Button type="button" variant="outline" size="sm" disabled={!!restoringAsset} onClick={() => restore(asset)}>
                    {restoringAsset === asset.id && <LoaderCircle className="animate-spin" />}
                    Restore
                  </Button>
                </div>
              </div>
            ))}
          </CardContent>
        </Card>
      )}

      <AssetFormDialog
        asset={editingAsset}
        open={assetFormOpen}
        onOpenChange={setAssetFormOpen}
        onMutated={revalidateAssets}
      />
      <ActivityDialog
        asset={activityAsset}
        open={!!activityAsset}
        onOpenChange={(open) => {
          if (!open) {
            setActivityAsset(undefined)
            setActivityEdit(undefined)
          }
        }}
        interest={interestRequest.data}
        interestError={interestRequest.error}
        interestLoading={interestRequest.isLoading}
        edit={activityEdit}
        onMutated={revalidateAssets}
      />
      <HistoryDialog
        asset={historyAsset}
        open={!!historyAsset}
        onOpenChange={(open) => !open && setHistoryAsset(undefined)}
        onMutated={revalidateAssets}
        readOnly={!!historyAsset?.archived_at}
        onEdit={(item) => {
          if (!historyAsset || !item.edit) return
          setHistoryAsset(undefined)
          setActivityEdit(item.edit)
          setActivityAsset(historyAsset)
        }}
      />

      <Dialog open={!!archiveAsset} onOpenChange={(open) => !open && setArchiveAsset(undefined)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Archive {archiveAsset?.name}?</DialogTitle>
            <DialogDescription>
              The current balance must be zero. After archive it leaves active views; recorded history stays and can be reviewed from the archived list.
            </DialogDescription>
          </DialogHeader>
          {archiveError && (
            <div role="alert" className="bg-destructive/10 text-destructive rounded-lg border border-destructive/20 p-3 text-sm">
              {archiveError}
            </div>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setArchiveAsset(undefined)}>
              Cancel
            </Button>
            <Button type="button" variant="destructive" disabled={isArchiving} onClick={archive}>
              {isArchiving ? <LoaderCircle className="animate-spin" /> : <Trash2 />}
              Archive asset
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}

export default Assets
