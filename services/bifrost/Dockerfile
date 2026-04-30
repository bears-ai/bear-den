FROM maximhq/bifrost:v1.4.22

USER root
COPY config.json /app/default-config.json
COPY docker-entrypoint.bears.sh /usr/local/bin/docker-entrypoint.bears.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.bears.sh \
    && if command -v apk >/dev/null 2>&1; then apk add --no-cache python3; \
       elif command -v apt-get >/dev/null 2>&1; then apt-get update && apt-get install -y python3 && rm -rf /var/lib/apt/lists/*; \
       else echo "No supported package manager found for python3" >&2; exit 1; fi
USER appuser

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.bears.sh"]
