/*
 * sock-try -- send one request to faciolockd and print the verdict byte.
 *
 * The daemon's socket is SO_PEERCRED-gated to root, so every interesting path
 * through it needs a root client. `pam-try` is one, but it goes through libpam and therefore
 * needs the module installed and a pam.d file. This does not: it speaks the daemon's wire
 * format directly, which makes it the tool for watching the daemon's own behaviour rather
 * than the module's.
 *
 *   cc -O2 -o target/sock-try tools/sock-try.c
 *   sudo ./target/sock-try /run/faciolock.sock auth "$USER"
 *   sudo ./target/sock-try /run/faciolock.sock clear "$USER"
 *
 * Exits with the verdict number, so a shell can test it.
 *
 * NOT shipped and not a test harness. It is the thing that turns "pending" into "observed"
 * for the legs a `cargo test` cannot reach, and it is deliberately dumb: no retries, no
 * framing logic, no interpretation.
 */
#define _POSIX_C_SOURCE 200809L
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/socket.h>
#include <sys/un.h>

/* faciolock-ipc: version | op | timeout_ms le | username_len | username */
#define VERSION 1
#define HEADER 5
#define MAX_USERNAME 32

static const char *verdict_name(int v)
{
	switch (v) {
	case 0: return "Deny";
	case 1: return "Allow";
	case 2: return "Unavailable";
	case 3: return "NoEnrolment";
	case 4: return "RateLimited";
	case 5: return "Ack";
	default: return "(unknown -- not a verdict this protocol defines)";
	}
}

static int op_code(const char *name)
{
	if (!strcmp(name, "auth"))   return 1;
	if (!strcmp(name, "enroll")) return 2;
	if (!strcmp(name, "status")) return 3;
	if (!strcmp(name, "clear"))  return 4;
	return -1;
}

int main(int argc, char **argv)
{
	if (argc != 4) {
		fprintf(stderr, "usage: %s SOCKET auth|enroll|status|clear USER\n", argv[0]);
		return 64;
	}
	int op = op_code(argv[2]);
	size_t namelen = strlen(argv[3]);
	if (op < 0 || namelen == 0 || namelen > MAX_USERNAME) {
		fprintf(stderr, "sock-try: bad op or username\n");
		return 64;
	}

	int fd = socket(AF_UNIX, SOCK_SEQPACKET, 0);
	if (fd < 0) { perror("socket"); return 70; }

	struct sockaddr_un addr;
	memset(&addr, 0, sizeof addr);
	addr.sun_family = AF_UNIX;
	if (strlen(argv[1]) >= sizeof addr.sun_path) {
		fprintf(stderr, "sock-try: socket path too long\n");
		return 64;
	}
	strcpy(addr.sun_path, argv[1]);

	if (connect(fd, (struct sockaddr *)&addr, sizeof addr) < 0) {
		perror("connect");
		return 69;
	}

	unsigned char req[HEADER + MAX_USERNAME];
	req[0] = VERSION;
	req[1] = (unsigned char)op;
	req[2] = 0xB8; req[3] = 0x0B;   /* 3000 ms, little-endian */
	req[4] = (unsigned char)namelen;
	memcpy(req + HEADER, argv[3], namelen);

	if (write(fd, req, HEADER + namelen) < 0) { perror("write"); return 70; }

	unsigned char res[8];
	ssize_t n = read(fd, res, sizeof res);
	if (n == 0) {
		/* The protocol defines this as PAM_AUTHINFO_UNAVAIL on the caller's side. It is also
		 * what `serve::may_ask` refuses with: asking about somebody else, or asking for
		 * clear/enroll without being root, gets no reply at all rather than a Deny. */
		printf("no reply (EOF) -- refused, or the daemon closed on us\n");
		return 65;
	}
	if (n < 0) { perror("read"); return 70; }
	if (n != 4 || res[0] != VERSION) {
		printf("not a response: %zd bytes, version %u\n", n, res[0]);
		return 65;
	}

	unsigned elapsed = (unsigned)res[2] | ((unsigned)res[3] << 8);
	printf("%-12s (%u)   %u ms\n", verdict_name(res[1]), res[1], elapsed);
	return res[1];
}
