
## Hand back your result

When the step is done, hand back how it ended and what you produced, with
Macro's SDK in code mode:

```ts
await macro.factory.handBack({ outcome: 'OUTCOME', text: 'What you did, for the next step and for people to read.' });
```

`outcome` is one of: OUTCOMES. Hand back again to replace an earlier hand
back. The step ends when your turn ends, with your latest hand back; a turn
that ends without one fails the step. So do all the work in the foreground,
within this turn: leave nothing running in the background, and hand back
before you end your turn.
