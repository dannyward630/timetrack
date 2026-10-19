# Started-at-wins export boundary

Totals sum durations so overlapping entries count twice, but an export is a row list handed to someone else. We assign each entry to exactly one week/month by its `started_at`, never splitting or duplicating, because duplicating rows across files double-counts silently and splitting rewrites wall-clock the user entered; a 22:00→02:00 entry appears once with its full duration in the start period.
