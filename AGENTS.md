# Working on Briefcase

Read architecture/README.md, relevant ADRs and architecture/verification.yaml
before changing boundaries or verification commands. Use reviewed external
ARCHCTL_BIN and BRIEFCASE_BIN installations; see DISTRIBUTION.md. Candidate
builds are test subjects, never independent evidence producers for themselves.

Keep evidence, declarations, derived results and claims distinct. Preserve pinned
views, coverage limits and missing checks. Archclt maps Python imports; its gate
does not enforce Briefcase Rust boundaries. Run the reviewed behavioral commands
and retain omitted evaluation lanes as explicit gaps. Release artifacts exclude
local captures, validation outputs and dependency installations.
