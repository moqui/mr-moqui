---
name: dates
title: Dates and date phrases
description: Use when a request names a date, a period or a range (last month, this week, 3 days ago) for a find, a field or data.
kind: reference
aliases: [date, day, week, month, quarter, year, period, range, today, yesterday, time zone]
---
Write a date as `YYYY-MM-DD`, or `YYYY-MM-DD HH:mm` in the user's time zone, or as a phrase the server works out. The
answer shows what it used, as `resolved: {thruDate: "2026-09-30 23:59:59"}`; say that date to the user.

## Phrases
- `now`, `today`, `yesterday`, `tomorrow`.
- `N days ago`, `in N weeks`, with day, week, month or year.
- `start of last week`, `end of this month`, `beginning of next quarter`: day, week, month, quarter or year, with
  this, last or next.
- `last month` or `this year` alone is its start, or its end for a thru, to or end field.
The week starts on the day the tail's `now` line names.

## Ranges and find fields
- In `data`, a range is `{"orderDate": {"from": "start of last month", "thru": "end of last month"}}`, and a compare is
  `{"orderDate": ">=2026-01-01"}`.
- On a screen's find, a date field has `From` and `Thru` fields (`orderDate_from`, `orderDate_thru`).
- A date-period find field has a period (`_period`: day, week, month, quarter, year), an offset from the current one
  (`_poffset`: 0 this, -1 last, 1 next) and a date it counts from (`_pdate`); `_from` and `_thru` set the range
  directly. Prefer the period for "last month" and the range for named dates.
