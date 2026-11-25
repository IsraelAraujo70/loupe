"""Turns titles into URL slugs."""

import re
import unicodedata

MAX_SLUG_LENGTH = 40


def slugify(title: str, max_length: int = MAX_SLUG_LENGTH) -> str:
    """Lowercase ASCII words joined by single dashes, cut at a word boundary."""
    ascii_title = (
        unicodedata.normalize("NFKD", title).encode("ascii", "ignore").decode("ascii")
    )
    words = re.findall(r"[a-z0-9]+", ascii_title.lower())
    slug = ""
    for word in words:
        candidate = f"{slug}-{word}" if slug else word
        if len(candidate) > max_length:
            break
        slug = candidate
    return slug or "untitled"
