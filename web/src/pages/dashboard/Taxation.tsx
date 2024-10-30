// @ts-nocheck
import React from "react";
import useSwr from "swr";
import { BASE_URL, fetcher, formatCurrency, formatDate } from "../../util";
import { format } from "date-fns";
import { Card, Flex, Title, Text, List, ListItem, Grid } from "@tremor/react";

const Taxation = (props: {}) => {
  const { data, isLoading } = useSwr(`${BASE_URL}/taxation`, fetcher);

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
                  {Object.entries(taxItems)?.map(([key, value]) => (
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
