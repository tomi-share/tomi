# Demo: code file of a hidden item (Python).
# The code screen asks for biometrics or the passcode before showing anything.
import os

API_TOKEN = os.environ.get("TOMI_DEMO_TOKEN", "demo-token-not-a-real-secret")


def connect(host: str, port: int = 443) -> str:
    """Returns the connection URL."""
    return f"https://{host}:{port}/v1?token={API_TOKEN}"


if __name__ == "__main__":
    print(connect("relay.example.org"))
