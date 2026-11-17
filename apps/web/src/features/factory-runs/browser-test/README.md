# Runs browser regression

From `apps/web`, run:

```sh
bunx playwright test --config src/features/factory-runs/browser-test/playwright.config.ts
```

Set `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH` to use an existing Chromium, and
`FACTORY_RUNS_SCREENSHOT_DIR` to save desktop and phone screenshots.
`FACTORY_RUNS_BROWSER_PORT` overrides port 3019; the harness owns its Vite
server.

The fixture mounts the real side panel, Runs section and Run view on an
in-memory factory (`main.tsx`): one Run waits at a Gate, one runs (its
`write` Step ran two rounds of checks, the first failed), two have ended. Its cancel shows the Run ended half a second later, as a read would.
The tests, on desktop and on a phone with touch styles:

- answer the Gate with a note, cancel the Run after its confirmation, and
  start a new Run from a chosen Flow (Code, which needs a repository, isn't
  offered), checking that the Gate's question and answers are in view
  together and where focus lands;
- open `/?long-question`, whose Gate asks a 14-line question, type a long
  note, and check Approve and Cancel stay in view, the note stops growing,
  and the question and Steps can be reached;
- answer with a note of `fail` (a failed request: focus returns to the
  button), `conflict` (someone else answered first: the read the 409 brings
  takes the Gate away and focus moves to the status line) or `slow-fail`
  (fails after 200 ms, while the person moves on to Cancel run: focus stays
  put, and a sent cancel still moves focus once the Run reads as ended);
- confirm Cancel run, then press Approve: the Gate is locked, and focus moves
  to the status line once the Run reads as cancelled.

`insights.html` mounts Run insights on fixed numbers (`insights.tsx`); a read
takes 400 ms. `insights.browser.e2e.ts` checks the placeholder, then the
page, filters it to the code Flow and opens a Run from **Where runs stop**;
`?error` fails the first read (Retry reads again) and `?empty` has no Runs.
No page scrolls sideways, on desktop or on a phone.
