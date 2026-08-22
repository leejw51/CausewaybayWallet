"""Allow ``python -m causewaybay`` alongside the ``cwbwallet`` console script."""

from .cli import main

if __name__ == "__main__":
    raise SystemExit(main())
