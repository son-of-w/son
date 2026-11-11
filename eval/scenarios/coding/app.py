"""tidepool-api: a fictional service for the son evaluation."""
from routes import status

ROUTES = {
    "/status": status.handle,
}


def handle(path):
    handler = ROUTES.get(path)
    return handler() if handler else {"error": "not found"}


if __name__ == "__main__":
    print(handle("/status"))
