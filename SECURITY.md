# Security policy

## Reporting

Please report vulnerabilities privately through GitHub's
[private vulnerability reporting](https://github.com/masa-tichita/roomy/security/advisories/new),
not in public issues. We aim to reply within a week.

## What counts

roomy removes files, so **any way to make it delete or move something it should not** is treated
as a security issue, for example:

- a path that gets past the guard (outside the home folder, protected folders, symbolic links),
- a candidate that is removed although its situation changed after the scan,
- a rule or config value that leads to removing user data without a warning,
- the state files leaking information to other users.

## Supported versions

Fixes go into the latest release. Until 1.0, older versions are not patched.
