# syntax=docker/dockerfile:1
FROM python:3.12-alpine
WORKDIR /app
COPY preflight.py /app/preflight.py
ENTRYPOINT ["python3", "/app/preflight.py"]
