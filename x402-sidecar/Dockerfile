# Lightweight runtime image for the x402 sidecar.
# Two-stage to keep the final image small.
FROM python:3.12-slim AS builder

WORKDIR /build
COPY pyproject.toml ./
COPY README.md ./
COPY src/ ./src/

# Install into a dedicated prefix so the final stage only carries deps + our code.
RUN pip install --no-cache-dir --upgrade pip && \
    pip install --no-cache-dir --prefix=/install .

FROM python:3.12-slim AS runtime

# Non-root user for runtime safety.
RUN groupadd --system --gid 1001 sidecar && \
    useradd  --system --uid 1001 --gid sidecar --create-home sidecar

WORKDIR /app
COPY --from=builder /install /usr/local
COPY --from=builder /build/src /app/src

ENV PYTHONPATH=/app/src \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    SIDECAR_BIND_HOST=0.0.0.0 \
    SIDECAR_BIND_PORT=8402

USER sidecar

EXPOSE 8402

# Healthcheck hits the /health endpoint. Tolerant timing — startup may pull config.
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD python -c "import urllib.request,sys; sys.exit(0 if urllib.request.urlopen('http://localhost:8402/health',timeout=3).status==200 else 1)"

CMD ["uvicorn", "himari_x402_sidecar.app:app", \
     "--host", "0.0.0.0", "--port", "8402"]
