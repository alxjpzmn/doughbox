import useSwr from "swr";
import { Card, Title, Text, List, ListItem, Grid } from "@tremor/react";
import { BASE_URL, fetcher, formatCurrency, formatDate } from "@/util";

const Taxation = () => {
  const { data } = useSwr(`${BASE_URL}/taxation`, fetcher);

  return (
    <div>
      {!!data && (
        <div>
          <Text color="slate" className="mb-4">
            Report from {formatDate(new Date(data?.created_at))}
          </Text>
          <Grid className="grid-col-1 gap-4">
            {Object.entries(data?.data).map(([year, taxItems]) => (
              <Card key={year}>
                <Title>{year}</Title>
                <List>
                  {Object.entries(taxItems as any)?.map(([key, value]) => (
                    <ListItem key={`${key}`}>
                      {key}
                      <span className="font-bold">
                        {formatCurrency(value as number)}
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
