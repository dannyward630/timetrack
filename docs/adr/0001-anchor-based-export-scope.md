# Anchor-based export scope

Export needed current week, chosen week, chosen month, and all time (§10), but only current-week/month/all existed. We added `--at YYYY-MM-DD` (default now; `all` ignores it) with client parsing to `anchor_ms` and a new `ExportCsvAt(scope, anchor_ms)` alongside the old `ExportCsv(scope)` now-wrapper, because one anchor rule covers current/chosen/last without a second addressing scheme, and old clients keep working.
