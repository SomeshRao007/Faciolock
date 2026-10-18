# `pam-configs/faciolock`

Installed to `/usr/share/pam-configs/faciolock` and applied by `pam-auth-update`. Never edit
`/etc/pam.d/*` from a maintainer script — kscreenlocker's own
documentation says the failure mode is a screen you cannot unlock.

## Why this one file covers all six services

Every service Faciolock integrates with includes `common-auth` and `common-account` on
Kubuntu — verified rather than assumed:

```
/etc/pam.d/sudo   /etc/pam.d/su   /etc/pam.d/login   /etc/pam.d/sddm
/usr/lib/pam.d/kde   /usr/lib/pam.d/polkit-1
```

`pam-auth-update` edits those two common files, so the enabled profile reaches all six at
once and disabling it removes all six at once. `faciolockctl enable` and `disable` use that
tool and update the greeter setting; the System Settings switch calls the same commands.

## The four lines, and why each is the shape it is

| Line | Why |
|---|---|
| `Default: yes` | A fresh install enables the profile. Until a face is enrolled, every attempt falls through to the password prompt. An explicit `disable` remains disabled on upgrades. |
| `Priority: 512` | Above `pam_unix` at 256, so face is tried before the password prompt. |
| `[success=end default=ignore]` | The fail-open rule. **Every** non-success — daemon down, not enrolled, rate-limited, refused, panicked — falls through to the password stack. This is the single line that makes a broken Faciolock an inconvenience rather than a lockout. |
| `Account: optional` | **The lockout release needs it.** It lives in `pam_sm_acct_mgmt`: after any successful authentication — password included — the module sends `Op::Clear` and the failure budget resets. Without an account line the budget is never cleared, so three bad face attempts disable face unlock until the daemon is restarted. `optional` rather than `required` because the module returns `PAM_IGNORE` on every path by design: an account-stack failure refuses a login outright instead of falling through, and Faciolock must never be able to express an opinion about whether an account is valid. |
