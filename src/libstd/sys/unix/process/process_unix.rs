// Copyright 2014-2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use env;
use ffi::CString;
use io::{self, Error, ErrorKind};
use libc::{self, c_int, gid_t, pid_t, uid_t};
use ptr;

use sys::cvt;
use sys::process::process_common::*;

////////////////////////////////////////////////////////////////////////////////
// Command
////////////////////////////////////////////////////////////////////////////////

impl Command {
    pub fn spawn(&mut self, default: Stdio, needs_stdin: bool)
                 -> io::Result<(Process, StdioPipes)> {
        use sys;

        const CLOEXEC_MSG_FOOTER: &'static [u8] = b"NOEX";

        let envp = self.capture_env();

        if self.saw_nul() {
            return Err(io::Error::new(ErrorKind::InvalidInput,
                                      "nul byte found in provided data"));
        }

        let (ours, theirs) = self.setup_io(default, needs_stdin)?;

        if let Some(ret) = self.posix_spawn(&theirs, envp.as_ref())? {
            return Ok((ret, ours))
        }

        let possible_paths = self.compute_possible_paths(envp.as_ref());

        let (input, output) = sys::pipe::anon_pipe()?;

        let pid = unsafe {
            match cvt(libc::fork())? {
                0 => {
                    drop(input);
                    let err = self.do_exec(theirs, envp.as_ref(), possible_paths);
                    let errno = err.raw_os_error().unwrap_or(libc::EINVAL) as u32;
                    let bytes = [
                        (errno >> 24) as u8,
                        (errno >> 16) as u8,
                        (errno >>  8) as u8,
                        (errno >>  0) as u8,
                        CLOEXEC_MSG_FOOTER[0], CLOEXEC_MSG_FOOTER[1],
                        CLOEXEC_MSG_FOOTER[2], CLOEXEC_MSG_FOOTER[3]
                    ];
                    // pipe I/O up to PIPE_BUF bytes should be atomic, and then
                    // we want to be sure we *don't* run at_exit destructors as
                    // we're being torn down regardless
                    assert!(output.write(&bytes).is_ok());
                    libc::_exit(1)
                }
                n => n,
            }
        };

        let mut p = Process { pid: pid, status: None };
        drop(output);
        let mut bytes = [0; 8];

        // loop to handle EINTR
        loop {
            match input.read(&mut bytes) {
                Ok(0) => return Ok((p, ours)),
                Ok(8) => {
                    assert!(combine(CLOEXEC_MSG_FOOTER) == combine(&bytes[4.. 8]),
                            "Validation on the CLOEXEC pipe failed: {:?}", bytes);
                    let errno = combine(&bytes[0.. 4]);
                    assert!(p.wait().is_ok(),
                            "wait() should either return Ok or panic");
                    return Err(Error::from_raw_os_error(errno))
                }
                Err(ref e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => {
                    assert!(p.wait().is_ok(),
                            "wait() should either return Ok or panic");
                    panic!("the CLOEXEC pipe failed: {:?}", e)
                },
                Ok(..) => { // pipe I/O up to PIPE_BUF bytes should be atomic
                    assert!(p.wait().is_ok(),
                            "wait() should either return Ok or panic");
                    panic!("short read on the CLOEXEC pipe")
                }
            }
        }

        fn combine(arr: &[u8]) -> i32 {
            let a = arr[0] as u32;
            let b = arr[1] as u32;
            let c = arr[2] as u32;
            let d = arr[3] as u32;

            ((a << 24) | (b << 16) | (c << 8) | (d << 0)) as i32
        }
    }

    pub fn exec(&mut self, default: Stdio) -> io::Error {
        let envp = self.capture_env();

        if self.saw_nul() {
            return io::Error::new(ErrorKind::InvalidInput,
                                  "nul byte found in provided data")
        }

        let possible_paths = self.compute_possible_paths(envp.as_ref());
        match self.setup_io(default, true) {
            Ok((_, theirs)) => unsafe { self.do_exec(theirs, envp.as_ref(), possible_paths) },
            Err(e) => e,
        }
    }

    fn compute_possible_paths(&self, maybe_envp: Option<&CStringArray>) -> Option<Vec<CString>> {
        let program = self.get_program().as_bytes();
        if program.contains(&b'/') {
            return None;
        }
        // Outside the match so we can borrow it for the lifetime of the function.
        let parent_path = env::var("PATH").ok();
        let paths = match maybe_envp {
            Some(envp) => {
                match envp.get_items().iter().find(|var| var.as_bytes().starts_with(b"PATH=")) {
                    Some(p) => &p.as_bytes()[5..],
                    None => return None,
                }
            },
            // maybe_envp is None if the process isn't changing the parent's env at all.
            None => {
                match parent_path.as_ref() {
                    Some(p) => p.as_bytes(),
                    None => return None,
                }
            },
        };

        let mut possible_paths = vec![];
        for path in paths.split(|p| *p == b':') {
            let mut binary_path = Vec::with_capacity(program.len() + path.len() + 1);
            binary_path.extend_from_slice(path);
            binary_path.push(b'/');
            binary_path.extend_from_slice(program);
            let c_binary_path = CString::new(binary_path).unwrap();
            possible_paths.push(c_binary_path);
        }
        return Some(possible_paths);
    }

    // And at this point we've reached a special time in the life of the
    // child. The child must now be considered hamstrung and unable to
    // do anything other than syscalls really. Consider the following
    // scenario:
    //
    //      1. Thread A of process 1 grabs the malloc() mutex
    //      2. Thread B of process 1 forks(), creating thread C
    //      3. Thread C of process 2 then attempts to malloc()
    //      4. The memory of process 2 is the same as the memory of
    //         process 1, so the mutex is locked.
    //
    // This situation looks a lot like deadlock, right? It turns out
    // that this is what pthread_atfork() takes care of, which is
    // presumably implemented across platforms. The first thing that
    // threads to *before* forking is to do things like grab the malloc
    // mutex, and then after the fork they unlock it.
    //
    // Despite this information, libnative's spawn has been witnessed to
    // deadlock on both macOS and FreeBSD. I'm not entirely sure why, but
    // all collected backtraces point at malloc/free traffic in the
    // child spawned process.
    //
    // For this reason, the block of code below should contain 0
    // invocations of either malloc of free (or their related friends).
    //
    // As an example of not having malloc/free traffic, we don't close
    // this file descriptor by dropping the FileDesc (which contains an
    // allocation). Instead we just close it manually. This will never
    // have the drop glue anyway because this code never returns (the
    // child will either exec() or invoke libc::exit)
    unsafe fn do_exec(
        &mut self,
        stdio: ChildPipes,
        maybe_envp: Option<&CStringArray>,
        maybe_possible_paths: Option<Vec<CString>>,
    ) -> io::Error {
        use sys::{self, cvt_r};

        macro_rules! t {
            ($e:expr) => (match $e {
                Ok(e) => e,
                Err(e) => return e,
            })
        }

        if let Some(fd) = stdio.stdin.fd() {
            t!(cvt_r(|| libc::dup2(fd, libc::STDIN_FILENO)));
        }
        if let Some(fd) = stdio.stdout.fd() {
            t!(cvt_r(|| libc::dup2(fd, libc::STDOUT_FILENO)));
        }
        if let Some(fd) = stdio.stderr.fd() {
            t!(cvt_r(|| libc::dup2(fd, libc::STDERR_FILENO)));
        }

        if cfg!(not(any(target_os = "l4re"))) {
            if let Some(u) = self.get_gid() {
                t!(cvt(libc::setgid(u as gid_t)));
            }
            if let Some(u) = self.get_uid() {
                // When dropping privileges from root, the `setgroups` call
                // will remove any extraneous groups. If we don't call this,
                // then even though our uid has dropped, we may still have
                // groups that enable us to do super-user things. This will
                // fail if we aren't root, so don't bother checking the
                // return value, this is just done as an optimistic
                // privilege dropping function.
                let _ = libc::setgroups(0, ptr::null());

                t!(cvt(libc::setuid(u as uid_t)));
            }
        }
        if let Some(ref cwd) = *self.get_cwd() {
            t!(cvt(libc::chdir(cwd.as_ptr())));
        }

        // emscripten has no signal support.
        #[cfg(not(any(target_os = "emscripten")))]
        {
            use mem;
            // Reset signal handling so the child process starts in a
            // standardized state. libstd ignores SIGPIPE, and signal-handling
            // libraries often set a mask. Child processes inherit ignored
            // signals and the signal mask from their parent, but most
            // UNIX programs do not reset these things on their own, so we
            // need to clean things up now to avoid confusing the program
            // we're about to run.
            let mut set: libc::sigset_t = mem::uninitialized();
            if cfg!(target_os = "android") {
                // Implementing sigemptyset allow us to support older Android
                // versions. See the comment about Android and sig* functions in
                // process_common.rs
                libc::memset(&mut set as *mut _ as *mut _,
                             0,
                             mem::size_of::<libc::sigset_t>());
            } else {
                t!(cvt(libc::sigemptyset(&mut set)));
            }
            t!(cvt(libc::pthread_sigmask(libc::SIG_SETMASK, &set,
                                         ptr::null_mut())));
            let ret = sys::signal(libc::SIGPIPE, libc::SIG_DFL);
            if ret == libc::SIG_ERR {
                return io::Error::last_os_error()
            }
        }

        for callback in self.get_closures().iter_mut() {
            t!(callback());
        }

        // If the program isn't an absolute path, and our environment contains a PATH var, then we
        // implement the PATH traversal ourselves so that it honors the child's PATH instead of the
        // parent's. This mirrors the logic that exists in glibc's execvpe, except using the
        // child's env to fetch PATH.
        match maybe_possible_paths {
            Some(possible_paths) => {
                let mut pending_error = None;
                for path in possible_paths {
                    libc::execve(
                        path.as_ptr(),
                        self.get_argv().as_ptr(),
                        maybe_envp.map(|envp| envp.as_ptr()).unwrap_or_else(|| *sys::os::environ())
                    );
                    let err = io::Error::last_os_error();
                    match err.kind() {
                        io::ErrorKind::PermissionDenied => {
                            // If we saw a PermissionDenied, and none of the other entries in
                            // $PATH are successful, then we'll return the first EACCESS we see.
                            if pending_error.is_none() {
                                pending_error = Some(err);
                            }
                        },
                        // Errors which indicate we failed to find a file are ignored and we try
                        // the next entry in the path.
                        io::ErrorKind::NotFound | io::ErrorKind::TimedOut => {
                            continue
                        },
                        // Any other error means we found a file and couldn't execute it.
                        _ => {
                            return err;
                        }
                    }
                }
                if let Some(err) = pending_error {
                    return err;
                }
                return io::Error::from_raw_os_error(libc::ENOENT);
            },
            _ => {
                libc::execve(
                    self.get_argv()[0],
                    self.get_argv().as_ptr(),
                    maybe_envp.map(|envp| envp.as_ptr()).unwrap_or_else(|| *sys::os::environ())
                );
                return io::Error::last_os_error()
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "freebsd",
                  all(target_os = "linux", target_env = "gnu"))))]
    fn posix_spawn(&mut self, _: &ChildPipes, _: Option<&CStringArray>)
        -> io::Result<Option<Process>>
    {
        Ok(None)
    }

    // Only support platforms for which posix_spawn() can return ENOENT
    // directly.
    #[cfg(any(target_os = "macos", target_os = "freebsd",
              all(target_os = "linux", target_env = "gnu")))]
    fn posix_spawn(&mut self, stdio: &ChildPipes, envp: Option<&CStringArray>)
        -> io::Result<Option<Process>>
    {
        use mem;
        use sys;

        if self.get_cwd().is_some() ||
            self.get_gid().is_some() ||
            self.get_uid().is_some() ||
            self.env_saw_path() ||
            self.get_closures().len() != 0 {
            return Ok(None)
        }

        // Only glibc 2.24+ posix_spawn() supports returning ENOENT directly.
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            if let Some(version) = sys::os::glibc_version() {
                if version < (2, 24) {
                    return Ok(None)
                }
            } else {
                return Ok(None)
            }
        }

        let mut p = Process { pid: 0, status: None };

        struct PosixSpawnFileActions(libc::posix_spawn_file_actions_t);

        impl Drop for PosixSpawnFileActions {
            fn drop(&mut self) {
                unsafe {
                    libc::posix_spawn_file_actions_destroy(&mut self.0);
                }
            }
        }

        struct PosixSpawnattr(libc::posix_spawnattr_t);

        impl Drop for PosixSpawnattr {
            fn drop(&mut self) {
                unsafe {
                    libc::posix_spawnattr_destroy(&mut self.0);
                }
            }
        }

        unsafe {
            let mut file_actions = PosixSpawnFileActions(mem::uninitialized());
            let mut attrs = PosixSpawnattr(mem::uninitialized());

            libc::posix_spawnattr_init(&mut attrs.0);
            libc::posix_spawn_file_actions_init(&mut file_actions.0);

            if let Some(fd) = stdio.stdin.fd() {
                cvt(libc::posix_spawn_file_actions_adddup2(&mut file_actions.0,
                                                           fd,
                                                           libc::STDIN_FILENO))?;
            }
            if let Some(fd) = stdio.stdout.fd() {
                cvt(libc::posix_spawn_file_actions_adddup2(&mut file_actions.0,
                                                           fd,
                                                           libc::STDOUT_FILENO))?;
            }
            if let Some(fd) = stdio.stderr.fd() {
                cvt(libc::posix_spawn_file_actions_adddup2(&mut file_actions.0,
                                                           fd,
                                                           libc::STDERR_FILENO))?;
            }

            let mut set: libc::sigset_t = mem::uninitialized();
            cvt(libc::sigemptyset(&mut set))?;
            cvt(libc::posix_spawnattr_setsigmask(&mut attrs.0,
                                                 &set))?;
            cvt(libc::sigaddset(&mut set, libc::SIGPIPE))?;
            cvt(libc::posix_spawnattr_setsigdefault(&mut attrs.0,
                                                    &set))?;

            let flags = libc::POSIX_SPAWN_SETSIGDEF |
                libc::POSIX_SPAWN_SETSIGMASK;
            cvt(libc::posix_spawnattr_setflags(&mut attrs.0, flags as _))?;

            let envp = envp.map(|c| c.as_ptr())
                .unwrap_or_else(|| *sys::os::environ() as *const _);
            let ret = libc::posix_spawnp(
                &mut p.pid,
                self.get_argv()[0],
                &file_actions.0,
                &attrs.0,
                self.get_argv().as_ptr() as *const _,
                envp as *const _,
            );
            if ret == 0 {
                Ok(Some(p))
            } else {
                Err(io::Error::from_raw_os_error(ret))
            }
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
// Processes
////////////////////////////////////////////////////////////////////////////////

/// The unique id of the process (this should never be negative).
pub struct Process {
    pid: pid_t,
    status: Option<ExitStatus>,
}

impl Process {
    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    pub fn kill(&mut self) -> io::Result<()> {
        // If we've already waited on this process then the pid can be recycled
        // and used for another process, and we probably shouldn't be killing
        // random processes, so just return an error.
        if self.status.is_some() {
            Err(Error::new(ErrorKind::InvalidInput,
                           "invalid argument: can't kill an exited process"))
        } else {
            cvt(unsafe { libc::kill(self.pid, libc::SIGKILL) }).map(|_| ())
        }
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        use sys::cvt_r;
        if let Some(status) = self.status {
            return Ok(status)
        }
        let mut status = 0 as c_int;
        cvt_r(|| unsafe { libc::waitpid(self.pid, &mut status, 0) })?;
        self.status = Some(ExitStatus::new(status));
        Ok(ExitStatus::new(status))
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status))
        }
        let mut status = 0 as c_int;
        let pid = cvt(unsafe {
            libc::waitpid(self.pid, &mut status, libc::WNOHANG)
        })?;
        if pid == 0 {
            Ok(None)
        } else {
            self.status = Some(ExitStatus::new(status));
            Ok(Some(ExitStatus::new(status)))
        }
    }
}
