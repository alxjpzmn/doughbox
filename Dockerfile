FROM node:lts-alpine AS frontend
WORKDIR /app
COPY web /app/web
RUN npm install -g pnpm
RUN cd /app/web && pnpm i && pnpm run build:docker

FROM rust:latest AS backend
WORKDIR /app
COPY core /app
COPY --from=frontend /app/web/dist /app/dist
RUN cd /app && cargo build --release


FROM gcr.io/distroless/cc
WORKDIR /app
COPY --from=backend /app/target/release/doughbox /app
COPY --from=frontend /app/web/dist /app/dist

EXPOSE 8084

CMD ["./doughbox", "api", "--silent"]

