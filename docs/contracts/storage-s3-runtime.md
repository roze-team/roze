# S3-compatible storage runtime

Roze 1.x provides an AWS Signature V4 adapter for `StorageProvider::S3Compatible`.
It uses path-style endpoints and supports:

- `put_object`, `get_object`, `delete_object`, and `stat_object`;
- signed PUT/GET URLs with bounded expiry;
- tenant prefixes, upload validation, metadata, ETags, and endpoint ports in
  the canonical `Host` header.

Qiniu Kodo, Aliyun OSS, and Tencent COS remain explicit provider-SDK
boundaries. Their mutation methods fail closed until provider-specific signing
is implemented; unsigned compatibility URLs are not runtime evidence.

The adapter is unit-tested locally. MinIO/S3 integration and failure-recovery
evidence still require the Linux Docker reference runner. The authoritative
round-trip command is:

```bash
ROZE_TEST_S3_ENDPOINT=http://127.0.0.1:9000 \
  cargo test -p roze-storage s3_compatible_round_trip_against_real_service \
  -- --ignored --nocapture
```
