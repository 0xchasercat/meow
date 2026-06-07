---
dr: DR-NNNN
title: <one line>
date: YYYY-MM-DD
status: accepted          # proposed | accepted | superseded | reversed
supersedes: null          # DR id or null
constitution_ref: []      # invariant ids this touches, if any
---

## Context
The fork/unknown that was resolved. What made it a real decision.

## Decision
One paragraph: what was decided.

## Consequences
What this commits us to; what it rules out; how it's reversed if wrong.

## Alternatives considered
The roads not taken, briefly, and why.

<!--
The Decision Ledger is immutable knowledge — "nothing is re-asked". DRs are authored by the
AGENT, including when the human resolves/corrects a record in the Decision Inbox: the inbox
records the human's choice, the next agent writes the DR from it and links it back
(`decisions.json` record -> `dr`). A wrong DR is never edited; it's superseded by a new one.
-->
