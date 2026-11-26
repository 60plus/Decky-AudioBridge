# Renders the store artwork (assets/src/*.html) with headless Chromium.
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends chromium fonts-inter fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/*
