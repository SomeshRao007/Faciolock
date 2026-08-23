/*
 * pam-try — run one PAM authentication and print the raw return code.
 *
 * The exit criterion for Phase 3 (wiki/09) is about what a real libpam does with
 * pam_faciolock.so, and no Rust test can answer that: the module has to be dlopen'd by
 * libpam, out of a pam.d file, with a real pam_handle_t. This is the smallest program that
 * makes that happen.
 *
 *   cc -O2 -o target/pam-try tools/pam-try.c -l:libpam.so.0
 *   ./target/pam-try faciolock-test "$USER"          # as yourself: refused by SO_PEERCRED
 *   sudo ./target/pam-try faciolock-test "$USER"     # as root: the real topology
 *
 * `-l:libpam.so.0` rather than `-lpam` so this builds without libpam0g-dev, and the four
 * prototypes are declared here for the same reason — see crates/faciolock-pam/build.rs.
 *
 * NOT a test harness and not shipped. It exists so a human can watch the codes.
 */
#define _POSIX_C_SOURCE 200809L
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct pam_handle pam_handle_t;
struct pam_conv {
	int (*conv)(int, const void **, void **, void *);
	void *appdata_ptr;
};

extern int pam_start(const char *, const char *, const struct pam_conv *, pam_handle_t **);
extern int pam_authenticate(pam_handle_t *, int);
extern int pam_acct_mgmt(pam_handle_t *, int);
extern int pam_end(pam_handle_t *, int);
extern const char *pam_strerror(pam_handle_t *, int);

/* The module never converses (wiki/03 R8). If something asks, that is itself a finding. */
static int refuse(int n, const void **msg, void **resp, void *data)
{
	(void)n; (void)msg; (void)resp; (void)data;
	fprintf(stderr, "pam-try: a module tried to converse; refusing\n");
	return 19; /* PAM_CONV_ERR */
}

/* _pam_types.h, the codes Faciolock can return. */
static const char *name_of(int rc)
{
	switch (rc) {
	case 0:  return "PAM_SUCCESS        -> face authenticated";
	case 7:  return "PAM_AUTH_ERR       -> denied, falls through to the password";
	case 9:  return "PAM_AUTHINFO_UNAVAIL -> could not ask, falls through to the password";
	case 10: return "PAM_USER_UNKNOWN   -> not enrolled, falls through to the password";
	default: return "(not a code this module constructs)";
	}
}

int main(int argc, char **argv)
{
	if (argc != 3) {
		fprintf(stderr, "usage: %s SERVICE USER\n", argv[0]);
		return 2;
	}
	struct pam_conv conv = { refuse, NULL };
	pam_handle_t *pamh = NULL;

	int rc = pam_start(argv[1], argv[2], &conv, &pamh);
	if (rc != 0) {
		fprintf(stderr, "pam_start: %d\n", rc);
		return 1;
	}

	struct timespec a, b;
	clock_gettime(CLOCK_MONOTONIC, &a);
	rc = pam_authenticate(pamh, 0);
	clock_gettime(CLOCK_MONOTONIC, &b);
	double ms = (b.tv_sec - a.tv_sec) * 1e3 + (b.tv_nsec - a.tv_nsec) / 1e6;

	printf("pam_authenticate -> %d  %-24s  %.1f ms\n  %s\n",
	       rc, pam_strerror(pamh, rc), ms, name_of(rc));

	/*
	 * Phase 4: the account stack is how the lockout is released (wiki/03 R5, P4-b), and
	 * the property to watch is that pam_faciolock returns PAM_IGNORE here whatever the
	 * daemon does. PAM_IGNORE is 25 and does not appear in the stack's own result, so what
	 * this prints is the *stack's* answer -- with pam_permit alongside it in the account
	 * section, PAM_SUCCESS means faciolock declined to have an opinion, which is correct.
	 * The daemon's journal is where the Op::Clear shows up.
	 *
	 * Run unconditionally, including after a failed pam_authenticate: an application would
	 * not, but the thing being checked is that this call can never make things worse.
	 */
	clock_gettime(CLOCK_MONOTONIC, &a);
	int acct = pam_acct_mgmt(pamh, 0);
	clock_gettime(CLOCK_MONOTONIC, &b);
	ms = (b.tv_sec - a.tv_sec) * 1e3 + (b.tv_nsec - a.tv_nsec) / 1e6;

	printf("pam_acct_mgmt    -> %d  %-24s  %.1f ms\n  %s\n",
	       acct, pam_strerror(pamh, acct), ms,
	       acct == 0 ? "stack allowed the account; faciolock returned PAM_IGNORE"
	                 : "the account stack refused -- faciolock must not be the reason");

	pam_end(pamh, rc);
	return rc == 0 ? 0 : 1;
}
