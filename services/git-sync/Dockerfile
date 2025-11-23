FROM alpine:3.19

# Install git, inotify-tools, and bash
RUN apk add --no-cache \
    git \
    inotify-tools \
    bash \
    openssh-client \
    && rm -rf /var/cache/apk/*

# Create sync directory
RUN mkdir -p /data

# Copy sync script
COPY sync.sh /usr/local/bin/sync.sh
RUN chmod +x /usr/local/bin/sync.sh

# Set working directory
WORKDIR /data

# Health check - verify git repo exists
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
    CMD [ -d "/data/.git" ] || exit 1

# Run sync script
ENTRYPOINT ["/usr/local/bin/sync.sh"]
