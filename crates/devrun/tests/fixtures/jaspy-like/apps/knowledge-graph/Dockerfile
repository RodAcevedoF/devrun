FROM python:3.11-slim
WORKDIR /app
CMD ["python3.11", "-m", "uvicorn", "src.entrypoints.http.app:app", "--host", "0.0.0.0", "--port", "8080"]
