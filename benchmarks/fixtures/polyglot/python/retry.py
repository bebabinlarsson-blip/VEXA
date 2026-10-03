def parse_retry_header(value: str) -> int | None:
    """Return a non-negative retry delay in seconds."""
    candidate = value.strip()
    if not candidate.isdigit():
        return None
    seconds = int(candidate)
    return seconds if seconds >= 0 else None


def should_retry(status_code: int) -> bool:
    return status_code in {429, 503}

