# Broker approvals

`LiveApprovalsPanel` is the Security approval center. It reads paginated pending requests and saved decisions from `approval.list`, and sends once-only decisions with the exact fingerprint and a command ID to `approval.respond`. Pending counts use the service's filtered total, rather than the loaded page length.

The conversation displays the same live requests. Truncated arguments or previews cannot be approved; denials remain available. External runtime requests identify their instance and owning run. Request expiration and process loss remain service-owned; the UI never simulates a successful decision or a remembered session grant.

The former sample approval components are not routed by Security.
