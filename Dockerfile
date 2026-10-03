FROM node:24-slim

# xz: rozpakowanie ffmpeg; fonts-dejavu: tekst na planszach
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates xz-utils fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --omit=dev
COPY scripts ./scripts
COPY .env.example ./
RUN node --disable-warning=ExperimentalWarning scripts/setup.ts --no-env
COPY src ./src
COPY public ./public

ENV NODE_ENV=production \
    PORT=8080 \
    DATA_DIR=/app/data
VOLUME /app/data
EXPOSE 8080
CMD ["node", "--disable-warning=ExperimentalWarning", "src/server.ts"]
