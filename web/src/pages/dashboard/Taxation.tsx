import useSwr from "swr";
import { Card, Title, Text, List, ListItem, Grid } from "@tremor/react";
import { BASE_URL, fetcher, formatCurrency, formatDate } from "@/util";
import { AnnualTaxableAmounts, SecWac, TaxationReport, FxWac } from "@/types/core";
import { Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRoot, TableRow } from "@/components/Table";
import EmptyState, { EmptyStateVariants } from "@/components/EmptyState";
import { Skeleton } from "@/components/Skeleton";

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
        <Grid className="grid-col-1 gap-4">
          {/* Skeleton for Report Date */}
          <Skeleton className="h-6 w-1/4 mb-4" />

          {/* Skeleton for Annual Taxable Amounts */}
          {Array.from({ length: 6 }).map((_, index) => (
            <Card key={index}>
              <Skeleton className="h-6 w-1/4 mb-4" /> {/* Placeholder for Year */}
              <List>
                {Array.from({ length: 5 }).map((_, idx) => (
                  <ListItem key={idx} className="flex gap-2 py-2">
                    <Skeleton className="h-6 w-3/4" /> {/* Placeholder for Tax Item Label */}
                    <Skeleton className="h-6 w-1/4" /> {/* Placeholder for Tax Item Value */}
                  </ListItem>
                ))}
              </List>
            </Card>
          ))}

          {/* Skeleton for Instrument WAC */}
          <Card>
            <Skeleton className="h-6 w-1/4 mb-4" /> {/* Placeholder for "Instrument WAC" Title */}
            <TableRoot>
              <Table>
                <TableHead>
                  <TableRow>
                    <TableHeaderCell><Skeleton className="h-6 w-24" /></TableHeaderCell>
                    <TableHeaderCell><Skeleton className="h-6 w-24" /></TableHeaderCell>
                    <TableHeaderCell><Skeleton className="h-6 w-24" /></TableHeaderCell>
                    <TableHeaderCell><Skeleton className="h-6 w-24" /></TableHeaderCell>
                  </TableRow>
                </TableHead>
                <TableBody>
                  {Array.from({ length: 3 }).map((_, idx) => (
                    <TableRow key={idx}>
                      <TableCell><Skeleton className="h-6 w-48" /></TableCell>
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </TableRoot>
          </Card>

          {/* Skeleton for Currency WAC */}
          <Card>
            <Skeleton className="h-6 w-1/4 mb-4" /> {/* Placeholder for "Currency WAC" Title */}
            <TableRoot>
              <Table>
                <TableHead>
                  <TableRow>
                    <TableHeaderCell><Skeleton className="h-6 w-24" /></TableHeaderCell>
                    <TableHeaderCell><Skeleton className="h-6 w-24" /></TableHeaderCell>
                    <TableHeaderCell><Skeleton className="h-6 w-24" /></TableHeaderCell>
                  </TableRow>
                </TableHead>
                <TableBody>
                  {Array.from({ length: 3 }).map((_, idx) => (
                    <TableRow key={idx}>
                      <TableCell><Skeleton className="h-6 w-48" /></TableCell>
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </TableRoot>
          </Card>
        </Grid>
      ) : data && !!data.created_at && !!data.taxable_amounts ? (
        <div>
          <Text color="slate" className="mb-4">
            Report (🇦🇹) from {formatDate(new Date(data?.created_at))}
          </Text>
          <Grid className="grid-col-1 gap-4">
            {Object.entries(data?.taxable_amounts).map(([year, taxItems]) => (
              <Card key={year}>
                <Title>{year}</Title>
                <List>
                  {Object.entries(taxItems as AnnualTaxableAmounts)?.map(([key, value]) => (
                    <ListItem key={`${key}`}>
                      {labelMap[key as keyof AnnualTaxableAmounts]}
                      <span className="font-bold">
                        {formatCurrency(parseFloat(value))}
                      </span>
                    </ListItem>
                  ))}
                </List>
              </Card>
            ))}
            <Card>
              <Title>Instrument WAC</Title>
              <TableRoot>
                <Table>
                  <TableHead>
                    <TableRow>
                      <TableHeaderCell>Name</TableHeaderCell>
                      <TableHeaderCell>Units</TableHeaderCell>
                      <TableHeaderCell>WAC</TableHeaderCell>
                      <TableHeaderCell>WAC FX</TableHeaderCell>
                    </TableRow>
                  </TableHead>
                  <TableBody>
                    {Object.entries(data?.securities_wacs as SecWac[])?.map(([key, value]) => (
                      <TableRow key={key}>
                        <TableCell className="truncate overflow-hidden whitespace-nowrap max-w-48">
                          {value.name}
                        </TableCell>
                        <TableCell>{value.units}</TableCell>
                        <TableCell>{value.average_cost}</TableCell>
                        <TableCell>{value.weighted_avg_fx_rate}</TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </TableRoot>
            </Card>
            <Card>
              <Title>Currency WAC</Title>
              <TableRoot>
                <Table>
                  <TableHead>
                    <TableRow>
                      <TableHeaderCell>Name</TableHeaderCell>
                      <TableHeaderCell>Units</TableHeaderCell>
                      <TableHeaderCell>WAC</TableHeaderCell>
                    </TableRow>
                  </TableHead>
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
              </TableRoot>
            </Card>
          </Grid>
        </div>
      ) : null}
    </div>
  );
};

export default Taxation;

