# Secret fields are redacted when settings leave the server and kept on disk.
SECRET_FIELDS = {"api_token"}
# Per-provider secrets live inside each providers.<name> mapping.
PROVIDER_SECRET_FIELDS = {"api_key"}
