# Wave NPU Device Manifests

Manifests bind research claims to exact device and firmware evidence.

A marketing name or apparent firmware version is not enough for native NPU
work. The executable probe, model, userspace runtime, graph target, kernel,
and observable NPU state all matter. A manifest therefore records hashes and
evidence provenance rather than treating a filename as identity.

## Evidence values

Fields use one of these evidence labels:

- `observed-live` — returned by the connected Wave during the dated
  collection session;
- `static-verified` — derived from a named, hashed local firmware artifact or
  source file;
- `observed-and-static-verified` — live bytes and the local artifact agree;
- `inferred` — supported by evidence, but not directly identified by the
  device;
- `unknown` — deliberately unresolved.

An `inferred` firmware label must never be promoted to `observed-live` merely
because its library hashes match. It may be described as
“v1.1.6.6-compatible rootfs” while the product-level release identifier
remains unknown.

## Privacy

Tracked manifests do not include:

- ADB serial numbers;
- Wi-Fi or Bluetooth identifiers;
- device credentials or keys;
- audio samples;
- user data;
- complete process lists;
- complete kernel logs.

Use a stable research alias such as `wave-lab-unit-a`.

## Updating a manifest

Create a new manifest rather than silently changing an old baseline when any
of these changes:

- kernel identity;
- `libnnsdk.so`, `libovxlib.so`, `libArchModelSw.so`, or related runtime hash;
- model hash;
- NBG version or target product;
- probe hash;
- device hardware revision;
- behavior of the known semantic control.

A new observation on the same exact stack may append a dated validation
record, provided historical evidence remains intact.

## Required content

A useful manifest records:

- evidence cutoff and collection context;
- privacy-preserving target alias;
- kernel and rootfs identity;
- `/dev/galcore` identity and relevant read-only parameters;
- artifact path, byte size, SHA-256, role, and evidence;
- NBG version, product target, tensor metadata, and known output;
- PoC run IDs and definition hash;
- unchanged-state and cleanup evidence;
- uncertainties and explicit non-claims;
- bounded commands or harness version used to collect the facts.

## Validation

The files are ordinary JSON so they remain usable without third-party tools:

```bash
python3 -m json.tool \
  docs/npu/manifests/wave-live-v1.1.6.6.json >/dev/null
```

The executable gate remains `waved/scripts/npu/experiments.json`. A manifest
documents evidence; it does not itself authorize or construct a remote
command.

