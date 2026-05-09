# IPC protocol

The shim and the bridge subprocess exchange JSON messages over a pipe.
This document is the canonical reference; if any divergence ever exists
between this file and `crates/ipc/src/schema.rs`, the schema source is
authoritative and this file should be updated to match.

## Wire format

Each message on the pipe is:

```text
<u32 little-endian byte length><UTF-8 JSON body of that length>
```

`length` is the byte count of the JSON body (not its character count).
The hard cap is `MAX_MESSAGE_BYTES = 16 * 1024 * 1024` — the codec
rejects larger frames before any JSON parsing happens.

## Stderr

Stderr is **not** part of the IPC stream. The subprocess emits
structured log lines on stderr; the shim drains them on a background
thread and forwards them to its own logger.

## Message kinds

Three top-level message kinds, all length-prefixed and tagged by a
`kind` discriminator:

```jsonc
// Shim → subprocess
{ "kind": "request", "id": <u64>, "op": "...", "body": ... }

// Subprocess → shim, correlated by id
{ "kind": "response", "id": <u64>, "status": "ok"|"err", ... }

// Subprocess → shim, unsolicited
{ "kind": "event", "event": "annotation"|"log", "body": ... }
```

The shim's request id begins at 1 and increases monotonically. `id: 0`
is reserved for the shutdown sentinel — see below.

## Operations

### `list_decoders`

```jsonc
// Request
{ "kind": "request", "id": 1, "op": "list_decoders" }

// Response
{
  "kind": "response", "id": 1, "status": "ok",
  "body": [
    {
      "id": "sigrok.onewire",
      "display_name": "1-Wire",
      "description": "Maxim 1-Wire serial bus.",
      "channels": [
        {"name":"data","description":"Data line","required":true}
      ],
      "options": [],
      "annotations": [
        {"id":"reset","description":"Reset / presence pulse"},
        {"id":"rom","description":"ROM command"}
      ],
      "tags": ["embedded"]
    }
    /* one entry per decoder the bridge can host */
  ]
}
```

Called once per subprocess lifetime. The shim caches the result for
the entire plugin load.

### `create_session`

```jsonc
{
  "kind": "request", "id": 7, "op": "create_session",
  "body": {
    "decoder_id": "sigrok.jtag",
    "channels": {"tck": 0, "tms": 1, "tdi": 2, "tdo": 3},
    "options": {"format": "auto"},
    "xz_policy": "glitch"
  }
}

// Response
{
  "kind":"response","id":7,"status":"ok",
  "body":{"session":"s1"}
}
```

`channels` maps **decoder channel name** to a **zero-based channel
index** in the upcoming sample frames. Required channels missing from
this map produce an `err` response with code `session_init_failed`.
Optional channels missing from this map are simply unused.

`xz_policy` is one of:

| Value | Meaning |
|---|---|
| `glitch` (default) | Indeterminate (X/Z) sample bits emit a `glitch` annotation and are not forwarded to libsigrokdecode. |
| `coerce_last` | Indeterminate bits are replaced by the previous determinate value of the same channel. |

### `feed`

```jsonc
{
  "kind":"request","id":9,"op":"feed",
  "body":{
    "session":"s1",
    "samples":[
      {
        "t":[
          {"fs": 0,        "set": [{"ch":0,"v":"one"}, {"ch":1,"v":"zero"}]},
          {"fs": 100000,   "set": [{"ch":0,"v":"x"}]}
        ]
      }
    ]
  }
}

// Response (annotations are unsolicited Events that arrive
// *before* this response — see "Event ordering" below)
{ "kind":"response","id":9,"status":"ok" }
```

`samples[*].t[*]` is one transition record. Each record names only the
channels that *changed* at that timestamp; channels not listed retain
their previous level. `fs` is femtoseconds since the session origin
(matches the C ABI's `WcSample.timestamp_fs`).

Bit values are `"zero"`, `"one"`, or `"x"`. The wire never carries `"z"`
— libsigrokdecode does not distinguish X from Z, so the shim collapses
both to `"x"` before sending.

### `finalize`

```jsonc
{
  "kind":"request","id":11,"op":"finalize",
  "body":{"session":"s1"}
}

{ "kind":"response","id":11,"status":"ok" }
```

Tell the bridge no more samples are coming for this session. The bridge
drains the decoder's internal state and may emit final annotations.

### `destroy`

```jsonc
{
  "kind":"request","id":12,"op":"destroy",
  "body":{"session":"s1"}
}

{ "kind":"response","id":12,"status":"ok" }
```

Release the session. After this, the session id is invalid.

### `shutdown`

```jsonc
{ "kind":"request","id":0,"op":"shutdown" }

{ "kind":"response","id":0,"status":"ok" }
```

The subprocess responds and exits. The shim's `Drop` impl sends this
on plugin teardown; if the subprocess hasn't exited within 2 seconds,
the shim sends `SIGKILL` (or `TerminateProcess` on Windows).

## Events

### `annotation`

Unsolicited; emitted by the subprocess as decoders produce output.

```jsonc
{
  "kind":"event","event":"annotation",
  "body":{
    "session":"s1",
    "start_fs": 1000000,
    "end_fs":   2000000,
    "ann_class": 2,
    "label": "IR=0x06 SAMPLE/PRELOAD",
    "fields": {"opcode":"0x06","ir_value":6},
    "is_error": false
  }
}
```

`ann_class` is the index in the decoder manifest's `annotations` list.

### `log`

Non-fatal log lines from inside libsigrokdecode or the subprocess. The
shim funnels these into its own structured logger; the WaveCrux
diagnostics surface displays them.

```jsonc
{
  "kind":"event","event":"log",
  "body":{"session":"s1","level":"warn","message":"crc mismatch"}
}
```

## Event ordering

Annotation events resulting from a `feed` or `finalize` request are
emitted **before** the matching `response` for that request. This lets
the shim — which returns from a request as soon as it sees the matching
response — be sure that every annotation triggered by a request has
already been buffered by the time the response arrives.

There is no other guarantee about event ordering: events from
different sessions running concurrently within the subprocess may
arrive in any order with respect to each other.

## Error responses

```jsonc
{
  "kind":"response","id":<u64>,"status":"err",
  "code":"<see below>",
  "message":"human-readable description"
}
```

| `code` | Meaning |
|---|---|
| `unknown_decoder` | `decoder_id` not found in the catalog. |
| `unknown_session` | `session` not found. |
| `session_init_failed` | Required channel missing, option invalid, or libsigrokdecode rejected the configuration. |
| `decoder_failed` | libsigrokdecode raised an error during decode. The session is in an undefined state; the shim destroys it. |
| `internal` | Unrecoverable internal error in the subprocess. The shim should mark every active session failed and re-spawn. |
| `bad_request` | The JSON parsed but failed semantic validation. |

## Backward compatibility

The IPC protocol is versioned implicitly: every field is optional on
the *receive* side wherever doing so is meaningful (`#[serde(default)]`
in the Rust types). Adding a new optional field is backwards
compatible. Adding a new request `op` requires both ends to be updated
together — the shim and the subprocess are versioned and shipped as a
single archive, so this is straightforward.

If a future minor version needs to add a request `op` while keeping
older shims working, the subprocess responds with
`err{code: bad_request}` for unknown ops and the shim continues to use
the older surface.
