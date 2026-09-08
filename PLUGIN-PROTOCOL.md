<!-- Defines the executable middleware contract implemented by Cargo Wrapper's plugin host. -->
# Cargo Wrapper plugin protocol v1

Cargo Wrapper plugins are native executables invoked as middleware. A plugin can inspect or transform a Cargo invocation, reject it without continuing, or perform checks after the downstream process returns. There is no stable Rust ABI, dynamic loading, network installer, or serialization dependency in protocol v1.

## Design

This protocol follows HashiCorp's well-established out-of-process executable-plugin design pattern, used by Terraform providers and other HashiCorp products. It does not implement HashiCorp's `go-plugin` protocol: Cargo Wrapper needs a lighter middleware contract, so protocol v1 passes Cargo arguments and versioned environment variables between ordinary subprocesses through `CARGO_WRAPPER_NEXT`, without RPC or gRPC.

## Invocation contract

The host launches each enabled plugin in ascending plugin-name order. It passes Cargo's arguments without `argv[0]`, preserves the working directory and inherited terminal streams, and supplies:

```text
CARGO_WRAPPER_PLUGIN_PROTOCOL=1
CARGO_WRAPPER_PLUGIN_NAME=<validated installed name>
CARGO_WRAPPER_REAL_CARGO=<absolute real Cargo path>
CARGO_WRAPPER_NEXT=<absolute Cargo Wrapper path>
CARGO_WRAPPER_ORIGINAL_SUBCOMMAND=<subcommand before wrapper rewrites>
```

The host also maintains these reserved continuation variables:

```text
CARGO_WRAPPER_PLUGIN_CHAIN=<comma-separated plugin names>
CARGO_WRAPPER_PLUGIN_INDEX=<next zero-based index>
CARGO_WRAPPER_PLUGIN_ROOT=<absolute captured plugin root>
CARGO_WRAPPER_PLUGIN_SESSION=<per-invocation diagnostic cookie>
```

`CARGO_WRAPPER_ORIGINAL_SUBCOMMAND` is absent when no subcommand was supplied. The session cookie helps diagnostics and recursion detection; it is not an authentication or authorization mechanism.

## Required plugin behavior

A conforming plugin must:

1. Act as middleware only when `CARGO_WRAPPER_PLUGIN_PROTOCOL` is exactly `1`.
2. Invoke `CARGO_WRAPPER_NEXT` with the final approved Cargo arguments so the remaining plugins run.
3. Preserve all reserved continuation variables when it invokes next.
4. Exit without invoking next when it rejects or completely handles an operation.
5. Normally invoke next no more than once and propagate its exit status, unless post-validation requires a stricter failure.
6. Never invoke `cargo` by name through `PATH`, which could recurse into the wrapper.

`CARGO_WRAPPER_REAL_CARGO` is for private Cargo operations that deliberately bypass later plugins, such as candidate resolution. A plugin's changes to the environment of its `CARGO_WRAPPER_NEXT` child propagate to inner plugins and final Cargo, but cannot constrain another plugin's deliberate direct-Cargo calls.

## Host guarantees

The wrapper applies its built-in argument policy once, before the first plugin. Continuation calls do not repeat that policy or parse plugin-management commands. The enabled chain is captured once per top-level invocation; before every launch, the host verifies that the named entry is still enabled and is a regular executable beneath the captured plugin root. A broken active entry fails closed.

After the last plugin, the host removes every public and reserved protocol variable before executing real Cargo. It preserves normal exit codes; a launch failure or termination without an exit code returns failure and never falls back to unwrapped Cargo.

Plugin management (`cargo wrapper plugin ...`) never executes plugins. Installing a plugin accepts only a local binary, copies it to host-managed storage, and leaves it disabled.

## Compatibility limits

Ordering by name is observable v1 behavior, so plugins whose policies depend on order must document their required registration names and ordering constraints. Protocol v1 has no priorities, remote catalog, automatic updates, plugin-owned configuration management, or guarantee for invoking next multiple times. Those require a later protocol revision.
