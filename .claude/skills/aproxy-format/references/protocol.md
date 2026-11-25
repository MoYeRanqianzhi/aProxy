# Format protocol 1

The complete contract between aProxy and a format program. Read it when writing a program,
checking a field, or working out why aProxy rejected a reply. In the samples, `->` is a line
aProxy writes to the program's stdin and `<-` is the program's reply on stdout; each is a single
line on the wire.

## Parts

This reference is split by topic into the files below (paths relative to this file). Open only the part the task needs; a reference elsewhere to `protocol.md, "<section>"` means the part that holds that section.

| File | Covers | Sections |
|---|---|---|
| [protocol/transport.md](protocol/transport.md) | How a program is started and talks to aProxy: transport, `hello`, common fields, the chain of programs. | [Transport](protocol/transport.md#transport) · [hello](protocol/transport.md#hello) · [Common fields](protocol/transport.md#common-fields) · [The chain](protocol/transport.md#the-chain) |
| [protocol/request-stages.md](protocol/request-stages.md) | Messages on the way upstream and the retry decision. | [request and retry](protocol/request-stages.md#request-and-retry) · [fail](protocol/request-stages.md#fail) · [decide](protocol/request-stages.md#decide) |
| [protocol/response-stages.md](protocol/response-stages.md) | Messages on the way back to the client and the notifications. | [response](protocol/response-stages.md#response) · [stream](protocol/response-stages.md#stream) · [heartbeat](protocol/response-stages.md#heartbeat) · [notify](protocol/response-stages.md#notify) · [report](protocol/response-stages.md#report) |
| [protocol/control-and-errors.md](protocol/control-and-errors.md) | Control methods, forward versus retry delivery, `required` and the circuit breaker, security rules. | [control](protocol/control-and-errors.md#control) · [Delivery: forward and retry](protocol/control-and-errors.md#delivery-forward-and-retry) · [Errors, `required` and the circuit breaker](protocol/control-and-errors.md#errors-required-and-the-circuit-breaker) · [Security](protocol/control-and-errors.md#security) |
