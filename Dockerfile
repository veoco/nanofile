# Nanofile - multi-stage Docker build
# Supports linux/amd64, linux/arm64 via Docker buildx.

# ── Builder stage ───────────────────────────────────────────────────
FROM node:22-alpine AS tailwind
WORKDIR /src
COPY static/css/input.css static/css/input.css
RUN npx --yes @tailwindcss/cli -i static/css/input.css -o /tmp/app.css --minify

FROM rust:alpine AS build
RUN apk add --no-cache build-base

# Build Tailwind CSS
COPY --from=tailwind /tmp/app.css /src/static/css/app.css

WORKDIR /src
COPY . .
RUN cargo build --release

# ── Runtime stage ────────────────────────────────────────────────────
FROM scratch
COPY --from=build /src/target/release/nanofile /nanofile
EXPOSE 8082
ENTRYPOINT ["/nanofile"]
