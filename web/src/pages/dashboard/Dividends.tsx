import useSwr from "swr";
import { Card, Metric, Subtitle, List, ListItem } from "@tremor/react";
import { BASE_URL, formatCurrency, fetcher, formatDate } from "@/util";

const Dividends = ({ }) => {
  const { data, isLoading } = useSwr(`${BASE_URL}/dividends`, fetcher);

  let total_dividends = data?.reduce((acc: any, item: any) => {
    return acc + parseFloat(item?.amount_eur);
  }, 0);

  return (
    <div className="min-h-screen">
      <Card className="mb-6">
        <Subtitle>Total dividends</Subtitle>
        <div>
          {isLoading ? (
            <>Loading...</>
          ) : (
            <Metric>{formatCurrency(total_dividends)}</Metric>
          )}
        </div>
      </Card>
      <Card>
        <List>
          {data?.map((item: any) => (
            <ListItem key={`${item?.isin} ${item?.date} ${item?.amount_eur}`}>
              <span>{formatDate(new Date(item?.date))}</span>
              < a href={`https://duckduckgo.com/?q=${item?.isin}`}>
                {item?.isin}
              </a>
              <span className="font-bold">
                {formatCurrency(item?.amount_eur)}
              </span>
            </ListItem>
          ))}
        </List>
      </Card>
    </div>
  );
};

export default Dividends;
