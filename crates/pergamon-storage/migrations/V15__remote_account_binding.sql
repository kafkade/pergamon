CREATE TABLE remote_account_binding (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    relay_url TEXT NOT NULL,
    local_label TEXT NOT NULL,
    content_account_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    flow TEXT NOT NULL CHECK (flow IN ('create', 'attach', 'join')),
    auth_tenant_id TEXT,
    server_instance_id TEXT,
    binding_version INTEGER,
    state TEXT NOT NULL CHECK (state IN ('pending', 'active'))
);
