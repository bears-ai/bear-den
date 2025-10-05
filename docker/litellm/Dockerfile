FROM python:3.11-slim

# Install LiteLLM with the gateway extras so the CLI server entrypoint is available.
ENV PIP_NO_CACHE_DIR=1

RUN pip install "litellm[proxy]" backoff

WORKDIR /app

# Use the litellm CLI as the image entrypoint so docker-compose commands still apply.
ENTRYPOINT ["litellm"]
