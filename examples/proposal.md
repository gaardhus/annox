# Proposal: Moving the team wiki to plain Markdown

## Summary

Our wiki currently lives in a hosted service that we pay for per seat. Most
pages are edited by two or three people, and almost nobody uses the
features that set it apart from a folder of text files. This proposal
recommends moving the wiki into a Git repository as plain Markdown, reviewed
through the same pull requests we use for code.

## Motivation

The hosted wiki has three problems:

1. **Cost.** We pay for 40 seats, but only 12 people edited a page last
   quarter.
2. **Drift.** Pages describe how systems worked when they were written.
   Nothing ties a page to the code it describes, so nobody notices when it
   goes stale.
3. **Review.** Edits go live immediately. There is no way to propose a
   change and have someone else look at it first.

Keeping documentation next to the code solves all three. It is free, it
changes in the same commits as the code, and every change is reviewed.

## Plan

We will export every page to Markdown using the wiki's built-in exporter,
commit the result to a new `docs/` directory, and fix broken links by hand.
The migration should take about a week. After that, the hosted wiki becomes
read-only for one month and is then shut down.

## Risks

Non-engineers may find Git intimidating. We can mitigate this by allowing
edits through the web interface of our Git host, which needs no local setup.

Search will be worse at first, since the hosted wiki has full-text search
built in. A static site generator would give us search back, but that is
out of scope for now.

## Decision

We ask the team to approve the migration by the end of the month.
