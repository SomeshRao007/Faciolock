# `pam-configs/faciolock`

Installed to `/usr/share/pam-configs/faciolock` and applied by `pam-auth-update`. Never edit
`/etc/pam.d/*` from a maintainer script — wiki/07 says why, and kscreenlocker's own
documentation says the failure mode is a screen you cannot unlock.

## Why this one file covers all six services

Every service in wiki/07's integration table includes `common-auth` and `common-account` on
this machine — verified rather than assumed:

```
/etc/pam.d/sudo   /etc/pam.d/su   /etc/pam.d/login   /etc/pam.d/sddm
/usr/lib/pam.d/kde   /usr/lib/pam.d/polkit-1
```

`pam-auth-update` edits those two common files, so enabling the profile reaches all six at
once and disabling it removes all six at once. That is the property hand-edited `pam.d` files
do not have, and it is also the reason a **sudo-only** trial cannot use this profile: there is
no such thing as enabling it for one service. A first trial belongs in `/etc/pam.d/sudo`
directly — see wiki/07, *Rolling it out, and getting back out*.

## The four lines, and why each is the shape it is

| Line | Why |
|---|---|
| `Default: no` | A face-auth package must never enable itself on install. Enrol first, then enable. |
| `Priority: 512` | Above `pam_unix` at 256, so face is tried before the password prompt. |
| `[success=end default=ignore]` | The fail-open rule. **Every** non-success — daemon down, not enrolled, rate-limited, refused, panicked — falls through to the password stack. This is the single line that makes a broken Faciolock an inconvenience rather than a lockout. |
| `Account: optional` | **Not in wiki/07's original sketch, and Phase 4 needs it.** The lockout release lives in `pam_sm_acct_mgmt` (P4-b): after any successful authentication — password included — the module sends `Op::Clear` and the failure budget resets. Without an account line the budget is never cleared, so three bad face attempts disable face unlock until the daemon is restarted. `optional` rather than `required` because the module returns `PAM_IGNORE` on every path by design: an account-stack failure refuses a login outright instead of falling through, and Faciolock must never be able to express an opinion about whether an account is valid. |
