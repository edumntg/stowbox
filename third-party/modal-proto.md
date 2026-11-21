# Modal client protocol definitions

`crates/stowbox-core/proto/modal/api.proto` and
`crates/stowbox-core/proto/modal/task_command_router.proto` are cut down
from `modal_proto/api.proto` and `modal_proto/task_command_router.proto` in
Modal's client repository, <https://github.com/modal-labs/modal-client>, at
commit `8dd0a049da4f3226688acacb5c74aaa405aff60c` (the Go SDK there is
0.11.2), downloaded on 2026-11-20.

Only the messages, fields, enum values and RPCs Stowbox uses are kept. Their
names, packages and field numbers are unchanged; `google.protobuf.Empty` is
replaced by an identical local `Empty`, and comments are dropped.

That repository is under the Apache License 2.0
(<https://www.apache.org/licenses/LICENSE-2.0>). Copyright Modal Labs, Inc.
