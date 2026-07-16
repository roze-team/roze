# Gate acknowledgements

Place narrowly scoped release-gate acknowledgement YAML files in this
directory and list them from `roze-gate.yaml`. A record must bind the exact old
and new SHA-256 contract digests and include an owner, reason, migration plan,
rollback plan, and expiry date.

Acknowledgements are temporary audit records, not compatibility shims. Remove
them after the migration completes and advance the reviewed baseline in the
same change.
