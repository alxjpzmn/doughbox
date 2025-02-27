import useSwr from "swr";
import { AnnualTaxableAmounts, SecWac, TaxationReport, FxWac } from "@/types/core";
import EmptyState, { EmptyStateVariants } from "@/components/composite/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { Disclaimer } from "@/components/composite/disclaimer";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from "@/components/ui/table";
import { BASE_URL, fetcher } from "@/lib/http";
import { formatCurrency } from "@/lib/utils";

const labelMap: Record<keyof AnnualTaxableAmounts, string> = {
  cash_interest: "Cash Interest",
  share_lending_interest: "Share Lending Interest",
  capital_gains: "Capital Gains",
  capital_losses: "Capital Losses",
  dividends: "Dividends",
  dividend_equivalents: "Dividend Equivalents",
  fx_appreciation: "FX Appreciation",
  withheld_tax_capital_gains: "Withheld Tax (Capital Gains)",
  withheld_tax_dividends: "Withheld Tax (Dividends)",
  withheld_tax_interest: "Withheld Tax (Interest)",
};

const Taxation = () => {
  const { data, error, isLoading } = useSwr<TaxationReport>(`${BASE_URL}/taxation`, fetcher);

  return (
    <div>
      {error && !error.details.events_present && <EmptyState variant={EmptyStateVariants.WithCliInstructionImport} docker={error.details?.in_docker} />}
      {error && error.details.events_present && <EmptyState variant={EmptyStateVariants.WithCliInstructionTaxation} docker={error.details?.in_docker} />}
      {isLoading ? (
        <div className="grid grid-cols-1 gap-4">
          {/* Skeleton for Report Date */}
          <Skeleton className="h-6 w-1/4 mb-4" />

          {/* Skeleton for Annual Taxable Amounts */}
          {Array.from({ length: 3 }).map((_, index) => (
            <Card key={index}>
              <CardHeader>
                <Skeleton className="h-6 w-1/4" /> {/* Placeholder for Year */}
              </CardHeader>
              <CardContent>
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead><Skeleton className="h-6 w-24" /></TableHead> {/* Placeholder for "Item" */}
                      <TableHead><Skeleton className="h-6 w-24" /></TableHead> {/* Placeholder for "Amount" */}
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {Array.from({ length: 5 }).map((_, idx) => (
                      <TableRow key={idx}>
                        <TableCell><Skeleton className="h-6 w-48" /></TableCell> {/* Placeholder for Tax Item Label */}
                        <TableCell><Skeleton className="h-6 w-24" /></TableCell> {/* Placeholder for Tax Item Value */}
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </CardContent>
            </Card>
          ))}

          {/* Skeleton for Instrument WAC */}
          <Card>
            <CardHeader>
              <Skeleton className="h-6 w-1/4" /> {/* Placeholder for "Instrument WAC" Title */}
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead> {/* Placeholder for "Name" */}
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead> {/* Placeholder for "Units" */}
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead> {/* Placeholder for "WAC" */}
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead> {/* Placeholder for "WAC FX" */}
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Array.from({ length: 3 }).map((_, idx) => (
                    <TableRow key={idx}>
                      <TableCell><Skeleton className="h-6 w-48" /></TableCell> {/* Placeholder for Name */}
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell> {/* Placeholder for Units */}
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell> {/* Placeholder for WAC */}
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell> {/* Placeholder for WAC FX */}
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </div>
      ) : (
        <div>
          {/* Annual Taxable Amounts */}
          {data?.taxable_amounts && Object.entries(data.taxable_amounts).map(([year, amounts]) => (
            <Card key={year} className="mb-4">
              <CardHeader>
                <CardTitle>{year}</CardTitle>
              </CardHeader>
              <CardContent>
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Item</TableHead>
                      <TableHead>Amount</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {Object.entries(amounts as AnnualTaxableAmounts).map(([key, value]) => (
                      <TableRow key={key}>
                        <TableCell>{labelMap[key as keyof AnnualTaxableAmounts]}</TableCell>
                        <TableCell>{formatCurrency(value)}</TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </CardContent>
            </Card>
          ))}

          {/* Instrument WAC */}
          <Card className="mb-4">
            <CardHeader>
              <CardTitle>Instrument WAC</CardTitle>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Units</TableHead>
                    <TableHead>WAC</TableHead>
                    <TableHead>WAC FX</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Object.entries(data?.securities_wacs as SecWac[])?.map(([key, value]) => (
                    <TableRow key={key}>
                      <TableCell className="truncate overflow-hidden whitespace-nowrap max-w-48">
                        {value.name}
                      </TableCell>
                      <TableCell>{value.units}</TableCell>
                      <TableCell>{formatCurrency(parseFloat(value.average_cost))}</TableCell>
                      <TableCell>{formatCurrency(parseFloat(value.weighted_avg_fx_rate))}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>

          {/* Currency WAC */}
          <Card>
            <CardHeader>
              <CardTitle>Currency WAC</CardTitle>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Units</TableHead>
                    <TableHead>WAC</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Object.entries(data?.currency_wacs as FxWac[])?.map(([key, value]) => (
                    <TableRow key={key}>
                      <TableCell>{key}</TableCell>
                      <TableCell>{value.units}</TableCell>
                      <TableCell>{value.avg_rate}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
          <Disclaimer />
        </div>
      )}
    </div>
  );
};

export default Taxation;
