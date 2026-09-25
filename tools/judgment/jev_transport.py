"""The sole network boundary for the on-demand Jev judgment tool."""

from __future__ import annotations

import json
import time
from urllib import error, request


ENDPOINT = "https://api.typesafe.ai/v1/systemone"


class TransportError(Exception):
    """A non-sensitive transport failure class."""


def send(payload: bytes, api_key: str, *, timeout: float = 30.0) -> dict:
    """POST canonical JSON bytes; retry only rate limiting and overload."""
    for attempt in range(3):
        message = request.Request(
            ENDPOINT,
            data=payload,
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
            },
            method="POST",
        )
        try:
            with request.urlopen(message, timeout=timeout) as response:
                if response.status != 200:
                    raise TransportError(f"http-{response.status}")
                body = response.read(1024 * 1024 + 1)
                if len(body) > 1024 * 1024:
                    raise TransportError("response-too-large")
                result = json.loads(body)
                if not isinstance(result, dict):
                    raise TransportError("invalid-response")
                return result
        except error.HTTPError as exc:
            if exc.code in {429, 529} and attempt < 2:
                time.sleep(0.5 * (2 ** attempt))
                continue
            raise TransportError(f"http-{exc.code}") from None
        except (error.URLError, TimeoutError, OSError):
            raise TransportError("network-error") from None
        except (ValueError, UnicodeError):
            raise TransportError("invalid-response") from None
    raise TransportError("retry-exhausted")
