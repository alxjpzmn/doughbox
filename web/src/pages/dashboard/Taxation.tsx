import useSwr from "swr";
import { Card, Title, Text, List, ListItem, Grid } from "@tremor/react";
import { BASE_URL, fetcher, formatCurrency, formatDate } from "@/util";
import { AnnualTaxableAmounts, SecWac, TaxationReport, Wac } from "@/types/core";
import { Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow } from "@/components/Table";

const Taxation = () => {
  const { data } = useSwr<TaxationReport>(`${BASE_URL}/taxation`, fetcher);
  console.log(data);


  return (
    <div>
      {(data && !!data.created_at && !!data.taxable_amounts) && (
        <div>
          <Text color="slate" className="mb-4">
            Report from {formatDate(new Date(data?.created_at))}
          </Text>
          <Grid className="grid-col-1 gap-4">
            {Object.entries(data?.taxable_amounts).map(([year, taxItems]) => (
              <Card key={year}>
                <Title>{year}</Title>
                <List>
                  {Object.entries(taxItems as AnnualTaxableAmounts)?.map(([key, value]) => (
                    <ListItem key={`${key}`}>
                      {key}
                      <span className="font-bold">
                        {formatCurrency(parseFloat(value))}
                      </span>
                    </ListItem>
                  ))}
                </List>
              </Card>
            ))}
            {<Card>
              <Title>Instrument WAC</Title>
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
                  {Object.entries(data?.securities_wacs as SecWac[])?.map(([key, value]) => <TableRow key={key}>
                    <TableCell className="max-w-6 md:max-w-fit truncate">{
                      key
                    }
                    </TableCell>
                    <TableCell>
                      {value.units}
                    </TableCell>
                    <TableCell>
                      {value.average_cost}
                    </TableCell>
                    <TableCell>
                      {value.weighted_avg_fx_rate}
                    </TableCell>
                  </TableRow>)}
                </TableBody>
              </Table>
            </Card>
            }
            {<Card>
              <Title>Currency WAC</Title>
              <Table>
                <TableHead>
                  <TableRow>
                    <TableHeaderCell>Name</TableHeaderCell>
                    <TableHeaderCell>Units</TableHeaderCell>
                    <TableHeaderCell>WAC</TableHeaderCell>
                  </TableRow>
                </TableHead>
                <TableBody>
                  {Object.entries(data?.currency_wacs as Wac[])?.map(([key, value]) => <TableRow key={key}>
                    <TableCell>{
                      key
                    }
                    </TableCell>
                    <TableCell>
                      {value.units}
                    </TableCell>
                    <TableCell>
                      {value.average_cost}
                    </TableCell>
                  </TableRow>)}
                </TableBody>
              </Table>
            </Card>
            }
          </Grid>
        </div>
      )
      }
    </div >
  )
};

export default Taxation;
