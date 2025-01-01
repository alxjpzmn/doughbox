import useSwr from "swr";
import { Card, Title, Text, List, ListItem, Grid } from "@tremor/react";
import { BASE_URL, fetcher, formatCurrency, formatDate } from "@/util";
import { AnnualTaxableAmounts, TaxationReport } from "@/types/core";

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
          </Grid>
        </div>
      )}
    </div>
  );
};

export default Taxation;
