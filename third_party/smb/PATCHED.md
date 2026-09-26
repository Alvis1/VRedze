Vendored smb 0.12.1 (MIT, see LICENSE; upstream https://github.com/afiffon/smb-rs) with one change for Just Video:
`Worker::receive` (src/connection/worker/worker_trait.rs) adds the credits granted
in interim STATUS_PENDING responses to the final response before credit accounting.
Samba grants an async request's credits in the interim response and none in the
final one, so unpatched every read that goes async (common under load: many 1 MiB
reads in flight, slow disks) leaked its credit charge. After a few thousand reads the
connection had no credits left and every request waited forever ("The server stopped
sending data"). Upstream candidate.
