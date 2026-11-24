"""Client for the Epochal API. Its one module today is the time portal: web search, web fetch and
sandboxes held to a past instant, so nothing published or crawled after it is used. Each method is
one API call, retried when that is safe and the failure may pass on its own.

    from epochal.api import Client

    with Client() as client:
        found = client.time_portal("2025-03-12").search("fed rate decision")
        for hit in found.results:
            print(hit.title, hit.url)

:class:`AsyncClient` does the same with coroutines. The API key comes from ``api_key=`` or
``EPOCHAL_API_KEY``.
"""

from epochal.api._async_client import AsyncClient
from epochal.api._base import MAX_WAIT
from epochal.api._client import Client
from epochal.api._errors import (
    APIError,
    DecodeError,
    EpochalError,
    InvalidArgumentError,
    MissingApiKeyError,
    TransportError,
)
from epochal.api._wire import Duration, Instant

__all__ = [
    "MAX_WAIT",
    "APIError",
    "AsyncClient",
    "Client",
    "DecodeError",
    "Duration",
    "EpochalError",
    "Instant",
    "InvalidArgumentError",
    "MissingApiKeyError",
    "TransportError",
]
