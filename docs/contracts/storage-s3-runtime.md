# S3-compatible storage runtime

Roze 1.x provides an AWS Signature V4 adapter for
`StorageProvider::S3Compatible` and `StorageProvider::QiniuKodo`.
It uses path-style endpoints and supports:

- `put_object`, `get_object`, `delete_object`, and `stat_object`;
- signed PUT/GET URLs with bounded expiry;
- tenant prefixes, upload validation, metadata, ETags, and endpoint ports in
  the canonical `Host` header.

Qiniu Kodo uses its official S3-compatible endpoint, region ID, S3 bucket
name, and AK/SK. The same runtime executes server-side PUT/GET/DELETE/HEAD and
signed PUT/GET URLs. Aliyun OSS and Tencent COS remain explicit provider-SDK
boundaries and their mutation methods fail closed.

The adapter is unit-tested locally. MinIO/S3 integration and failure-recovery
evidence still require the Linux Docker reference runner. The authoritative
round-trip command is:

```bash
ROZE_TEST_S3_ENDPOINT=http://127.0.0.1:9000 \
  cargo test -p roze-storage s3_compatible_round_trip_against_real_service \
  -- --ignored --nocapture
```

Real Kodo evidence is credential-gated. Use a dedicated test bucket and the
same round-trip operations with `provider: qiniu_kodo`; never place AK/SK in
source code or test output.
