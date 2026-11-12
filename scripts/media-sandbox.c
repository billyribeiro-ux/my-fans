/* SPDX-License-Identifier: MIT
 * Fail-closed Linux decoder confinement. No privileged namespace creation.
 * The invoking service owns a fresh 0700 scratch directory. No application
 * environment, inherited descriptor, socket, or secret path reaches execve.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/audit.h>
#include <linux/capability.h>
#include <linux/filter.h>
#include <linux/landlock.h>
#include <linux/seccomp.h>
#include <sched.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#if defined(__aarch64__)
#define NATIVE_ARCH AUDIT_ARCH_AARCH64
#elif defined(__x86_64__)
#define NATIVE_ARCH AUDIT_ARCH_X86_64
#else
#error Unsupported media sandbox architecture
#endif

static void fail(const char *stage) {
    /* Never include paths, argv, environment, or underlying secret contents. */
    fprintf(stderr, "media_sandbox_unavailable:%s\n", stage);
    _exit(125);
}
static void bound(int kind, rlim_t value) {
    struct rlimit r = {value, value};
    if (setrlimit(kind, &r)) fail("resource_limit");
}
static void allow_path(int ruleset, const char *path, uint64_t rights, int optional) {
    int fd = open(path, O_PATH | O_CLOEXEC);
    if (fd < 0) {if (optional && errno == ENOENT) return; fail("filesystem_rule");}
    struct landlock_path_beneath_attr rule = {.allowed_access = rights, .parent_fd = fd};
    if (syscall(SYS_landlock_add_rule, ruleset, LANDLOCK_RULE_PATH_BENEATH, &rule, 0)) fail("filesystem_rule");
    if (close(fd)) fail("descriptor_close");
}
#define DENY(n) BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_##n, 0, 1), BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM)
static void restrict_syscalls(void) {
    struct sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, arch)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, NATIVE_ARCH, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
#ifdef __x86_64__
        /* Reject the x32 syscall ABI rather than letting alternate numbers pass. */
        BPF_JUMP(BPF_JMP | BPF_JSET | BPF_K, 0x40000000U, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
#endif
        DENY(socket), DENY(socketpair), DENY(connect), DENY(bind), DENY(listen),
        DENY(accept), DENY(accept4), DENY(sendto), DENY(sendmsg), DENY(sendmmsg),
        DENY(recvfrom), DENY(recvmsg), DENY(recvmmsg),
        DENY(ptrace), DENY(process_vm_readv), DENY(process_vm_writev),
        DENY(pidfd_open), DENY(pidfd_getfd), DENY(pidfd_send_signal),
        DENY(io_uring_setup), DENY(io_uring_enter), DENY(io_uring_register),
        DENY(bpf), DENY(perf_event_open), DENY(userfaultfd),
        DENY(mount), DENY(umount2), DENY(pivot_root), DENY(chroot),
        DENY(unshare), DENY(setns), DENY(open_by_handle_at), DENY(name_to_handle_at),
        DENY(fsopen), DENY(fsmount), DENY(fsconfig), DENY(move_mount), DENY(open_tree),
        DENY(add_key), DENY(request_key), DENY(keyctl),
        DENY(init_module), DENY(finit_module), DENY(delete_module),
        DENY(kexec_load), DENY(reboot), DENY(swapon), DENY(swapoff),
        DENY(shmget), DENY(shmat), DENY(shmctl), DENY(shmdt),
        DENY(msgget), DENY(msgsnd), DENY(msgrcv), DENY(msgctl),
        DENY(semget), DENY(semctl), DENY(semop), DENY(semtimedop),
        DENY(kill), DENY(tkill), DENY(tgkill),
#ifdef SYS_fork
        DENY(fork),
#endif
#ifdef SYS_vfork
        DENY(vfork),
#endif
        /* pthread implementations retry clone when clone3 is unavailable. */
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_clone3, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | ENOSYS),
        /* Only threads are permitted. They inherit Landlock and this filter. */
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_clone, 0, 4),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, args[0])),
        BPF_JUMP(BPF_JMP | BPF_JSET | BPF_K, CLONE_THREAD, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program = {.len = (unsigned short)(sizeof(filter) / sizeof(filter[0])), .filter = filter};
    if (prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program)) fail("syscall_filter");
}
int main(int argc, char **argv) {
    if (argc < 3 || argc > 256 || getuid() == 0 || geteuid() != getuid() || getegid() != getgid()) fail("invocation");
    /* The two-rendition encoder needs 133 arguments including the launcher.
     * Bound both count and total bytes, including every terminating NUL. */
    size_t argument_bytes = 0;
    for (int i = 0; i < argc; i++) {
        size_t remaining = 32768 - argument_bytes;
        size_t length = strnlen(argv[i], remaining);
        if (length == remaining) fail("arguments");
        argument_bytes += length + 1;
    }
    struct __user_cap_header_struct capability_header = {.version = _LINUX_CAPABILITY_VERSION_3, .pid = 0};
    struct __user_cap_data_struct capabilities[2] = {{0},{0}};
    if (syscall(SYS_capget,&capability_header,capabilities)) fail("capabilities");
    for (size_t i=0;i<2;i++) if (capabilities[i].effective || capabilities[i].permitted || capabilities[i].inheritable) fail("capabilities");
    const char *binary = argv[2];
    if (strcmp(binary,"/usr/local/bin/ffmpeg") && strcmp(binary,"/usr/local/bin/ffprobe") && strcmp(binary,"/usr/local/bin/myfans-media")) fail("executable");
    /* No additional capabilities are needed, and setuid files cannot elevate. */
    pid_t supervisor = getppid();
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) || prctl(PR_SET_DUMPABLE, 0) || prctl(PR_SET_PDEATHSIG, SIGKILL)) fail("privilege_drop");
    if (getppid() != supervisor) fail("supervisor_lost");
    int abi = (int)syscall(SYS_landlock_create_ruleset, NULL, 0, LANDLOCK_CREATE_RULESET_VERSION);
    if (abi < 3) fail("landlock_abi");
    struct stat work_stat;
    int work = open(argv[1], O_PATH | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW);
    if (work < 0 || fstat(work,&work_stat) || work_stat.st_uid != getuid() || (work_stat.st_mode & 0777) != 0700) fail("scratch_permissions");
    if (fchdir(work) || close(work)) fail("scratch_directory");
    if (clearenv()) fail("environment");
    umask(0077);
    const uint64_t read = LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR;
    const uint64_t write = LANDLOCK_ACCESS_FS_WRITE_FILE | LANDLOCK_ACCESS_FS_REMOVE_DIR | LANDLOCK_ACCESS_FS_REMOVE_FILE | LANDLOCK_ACCESS_FS_MAKE_DIR | LANDLOCK_ACCESS_FS_MAKE_REG | LANDLOCK_ACCESS_FS_REFER | LANDLOCK_ACCESS_FS_TRUNCATE;
    struct landlock_ruleset_attr rules = {.handled_access_fs = LANDLOCK_ACCESS_FS_EXECUTE | read | write | LANDLOCK_ACCESS_FS_MAKE_CHAR | LANDLOCK_ACCESS_FS_MAKE_SOCK | LANDLOCK_ACCESS_FS_MAKE_FIFO | LANDLOCK_ACCESS_FS_MAKE_BLOCK | LANDLOCK_ACCESS_FS_MAKE_SYM};
    int ruleset = (int)syscall(SYS_landlock_create_ruleset, &rules, sizeof(rules), 0);
    if (ruleset < 0) fail("filesystem_policy");
    /* Outputs are flat files; even creating nested directories is unnecessary. */
    allow_path(ruleset,".",read|(write & ~LANDLOCK_ACCESS_FS_MAKE_DIR),0);
    allow_path(ruleset,binary,LANDLOCK_ACCESS_FS_READ_FILE|LANDLOCK_ACCESS_FS_EXECUTE,0);
    allow_path(ruleset,"/lib",read,0);
    allow_path(ruleset,"/usr/lib",read,0);
#if defined(__aarch64__)
    allow_path(ruleset,"/lib/ld-musl-aarch64.so.1",LANDLOCK_ACCESS_FS_READ_FILE|LANDLOCK_ACCESS_FS_EXECUTE,1);
    allow_path(ruleset,"/lib/ld-linux-aarch64.so.1",LANDLOCK_ACCESS_FS_READ_FILE|LANDLOCK_ACCESS_FS_EXECUTE,1);
#else
    allow_path(ruleset,"/lib/ld-musl-x86_64.so.1",LANDLOCK_ACCESS_FS_READ_FILE|LANDLOCK_ACCESS_FS_EXECUTE,1);
    allow_path(ruleset,"/lib64/ld-linux-x86-64.so.2",LANDLOCK_ACCESS_FS_READ_FILE|LANDLOCK_ACCESS_FS_EXECUTE,1);
#endif
    allow_path(ruleset,"/dev/null",LANDLOCK_ACCESS_FS_READ_FILE|LANDLOCK_ACCESS_FS_WRITE_FILE,0);
    allow_path(ruleset,"/dev/urandom",LANDLOCK_ACCESS_FS_READ_FILE,0);
    if (syscall(SYS_landlock_restrict_self,ruleset,0) || close(ruleset)) fail("filesystem_enforcement");
    /* Close everything except the explicit stdin/stdout/stderr pipes. */
    if (syscall(SYS_close_range,3,~0U,0)) fail("inherited_descriptors");
    bound(RLIMIT_CORE,0); bound(RLIMIT_AS,1024ULL*1024*1024); bound(RLIMIT_CPU,300);
    /* NPROC counts the real UID across the host, not this PID namespace. */
    bound(RLIMIT_FSIZE,512ULL*1024*1024); bound(RLIMIT_NOFILE,64); bound(RLIMIT_NPROC,256);
    restrict_syscalls();
    char *const environment[] = {"PATH=/usr/local/bin", "LANG=C", "TZ=UTC", NULL};
    execve(binary,&argv[2],environment);
    fail("execute");
}
