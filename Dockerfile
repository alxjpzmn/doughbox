FROM node:lts-alpine AS frontend
WORKDIR /app
COPY web /app/web
RUN npm install -g pnpm
RUN cd /app/web && pnpm i && pnpm run build:docker

FROM rust:latest AS backend
WORKDIR /app
COPY core /app
RUN cd /app && cargo build --release


FROM rust:latest
WORKDIR /app
COPY --from=backend /app/target/release /app
COPY --from=frontend /app/web/dist /app/dist

RUN apt-get update && apt-get install -y openssl libssl-dev

EXPOSE 8084

CMD ["./doughbox", "api", "--silent"]

