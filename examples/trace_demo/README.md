# Trace Demo

Demonstrates the trace-to-test workflow: record, generate, run, minimize.

## Record a trace

```
boruna trace2tests record examples/trace_demo/trace_app.ax \
  --messages "increment:0,increment:0,decrement:0,increment:0" \
  --out trace.json
```

## Generate a regression test

```
boruna trace2tests generate --trace trace.json --name counter_regression --out test.json
```

## Run the test

```
boruna trace2tests run --spec test.json --source examples/trace_demo/trace_app.ax
```

## Minimize a failing trace

```
boruna trace2tests minimize --trace trace.json --source examples/trace_demo/trace_app.ax --predicate panic
```

## Full pipeline

Record → Generate → Run ensures that any future change to the app that breaks the recorded behavior is caught as a regression.
