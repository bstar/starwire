# The stack

STAR/WIRE's window is one column of docked modules — sources, entries, reader
— where the one you are looking at is expanded and the ones behind it fold to
a line. Drilling in pushes a level; backing out pops it. It is the same shape
STAR/FOLD uses for directories, applied to feeds and articles.

**It is not built yet.** `starwire` with no arguments says so and exits; every
command in [On the command line](cli.md) works today. See
[Status](status.md) for what exists and what does not.

When it lands, this page will cover:

- The three modules, what each one holds, and how the spare rows are shared
  between them — including the "peek", where a focused list is capped so the
  article behind it stays visible.
- What pushes and what pops, and why the reader is a module rather than a
  stack level (so that stepping to the next entry keeps your place in the
  ones you have read).
- The special sources — everything, unread, starred, videos and a search —
  and why a search result is a level of the stack like any other.
- The first-run import overlay.

The design is settled; what is missing is the code.
