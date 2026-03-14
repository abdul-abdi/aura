FROM python:3.12-slim

WORKDIR /app

RUN adduser --disabled-password --gecos '' --uid 1001 aura

COPY requirements.lock .
RUN pip install --no-cache-dir -r requirements.lock

COPY config.py agent.py tools.py server.py ./

RUN chown -R aura:aura /app
USER aura

ENV PORT=8080
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["python", "-c", "import urllib.request; urllib.request.urlopen('http://localhost:8080/health')"]

CMD ["uvicorn", "server:app", "--host", "0.0.0.0", "--port", "8080"]
