# Example

A short proposal, [`proposal.md`](proposal.md), already reviewed by two
authors: a person (`mailto:sam@example.com`) and an agent
(`urn:agent:claude`). Its annotations, in [`.annox/`](.annox/), cover open,
approved, resolved, accepted, rejected, and orphaned threads, and a few
reactions.

To look at it:

- Open `proposal.md` in an editor with annox.
- Or run `annox report --verbose` in this directory.
- Or open [`proposal.annotated.pdf`](proposal.annotated.pdf) in a PDF
  viewer. It is the proposal compiled to a PDF, with its open threads
  exported as PDF comments by `annox export-pdf`.

[`proposal.pdf`](proposal.pdf) is the same PDF before the export. Run
`just example-pdf` from the repository root to rebuild both PDFs; it needs
pandoc and Typst.
