FROM maximhq/bifrost:v1.4.22

USER root
COPY config.json /app/default-config.json
COPY docker-entrypoint.bears.sh /usr/local/bin/docker-entrypoint.bears.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.bears.sh
USER appuser

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.bears.sh"]
