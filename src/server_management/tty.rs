use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsFd, AsRawFd},
        unix::process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

use nix::{
    fcntl::{FcntlArg, FdFlag, fcntl},
    pty::{Winsize, openpty},
    unistd::{setsid, ttyname},
};

// TIOCSWINSZ : change la taille du pty. Le noyau envoie ensuite SIGWINCH
// au groupe de processus au premier plan.
nix::ioctl_write_ptr_bad!(tiocswinsz, libc::TIOCSWINSZ, Winsize);

/// Construit une taille de terminal (en lignes / colonnes).
pub fn winsize(rows: u16, cols: u16) -> Winsize {
    Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

pub struct PtySession {
    /// Chemin du côté slave, par exemple `/dev/pts/3`.
    pub pts_path: PathBuf,
    pub child: Child,
    /// Côté master : on lit la sortie du programme et on écrit son entrée.
    pub master: File,
}

/// Lance `program` dans un nouveau pty, dont il devient le processus de session.
pub fn spawn_in_pty<I, S>(
    program: &Path,
    working_dir: &Path,
    args: I,
    size: Winsize,
) -> io::Result<PtySession>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let pty = openpty(Some(&size), None)?;

    // Empêche master et slave d'être hérités par les enfants. Les copies
    // dup2 vers 0/1/2 perdent CLOEXEC, donc le programme lancé garde bien
    // son stdin/stdout/stderr.
    fcntl(pty.master.as_fd(), FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
    fcntl(pty.slave.as_fd(), FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;

    let pts_path = ttyname(&pty.slave)?;

    let mut cmd = Command::new(program);
    cmd.current_dir(working_dir)
        .args(args)
        .env("TERM", "xterm-256color")
        .stdin(Stdio::from(pty.slave.try_clone()?))
        .stdout(Stdio::from(pty.slave.try_clone()?))
        .stderr(Stdio::from(pty.slave));

    // Nouvelle session, puis le pty devient le terminal de contrôle de
    // l'enfant (nécessaire pour Ctrl+C, le job control et /dev/tty).
    // stdin (fd 0) est déjà branché sur le slave quand ce code s'exécute.
    unsafe {
        cmd.pre_exec(|| {
            setsid()?;
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = cmd.spawn()?;

    // `cmd` détient encore les fds slave côté parent. Sans ce drop, le
    // master ne verrait jamais la fin du flux quand l'enfant se termine.
    drop(cmd);

    Ok(PtySession {
        pts_path,
        child,
        master: File::from(pty.master),
    })
}

impl PtySession {
    /// Change la taille du terminal (fonctionne à tout moment).
    pub fn resize(&self, size: Winsize) -> io::Result<()> {
        unsafe { tiocswinsz(self.master.as_raw_fd(), &size) }?;
        Ok(())
    }

    /// Envoie des octets à l'entrée du programme (comme si on tapait au clavier).
    pub fn write_input(&self, data: &[u8]) -> io::Result<()> {
        // `&File` implémente `Write`, donc pas besoin de `&mut self`.
        (&self.master).write_all(data)
    }

    /// Renvoie un lecteur indépendant sur la sortie du programme.
    /// La lecture est bloquante : à faire dans un thread ou un `spawn_blocking`.
    pub fn reader(&self) -> io::Result<PtyReader> {
        Ok(PtyReader(self.master.try_clone()?))
    }

    /// Tue tout le groupe de processus de l'enfant (lui et ses sous-processus).
    pub fn kill(&mut self) -> io::Result<()> {
        // L'enfant est leader de sa session (setsid), donc pgid == pid.
        let pgid = self.child.id() as libc::pid_t;
        if unsafe { libc::killpg(pgid, libc::SIGKILL) } == -1 {
            let err = io::Error::last_os_error();
            // ESRCH : plus rien à tuer, ce n'est pas une erreur.
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(err);
            }
        }
        Ok(())
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.kill();
            let _ = self.child.wait();
        }
    }
}

/// Lecteur sur le master du pty.
///
/// Sous Linux, quand le programme se termine, `read` renvoie `EIO` au lieu
/// d'un EOF propre. Ce wrapper le convertit en `Ok(0)`.
pub struct PtyReader(File);

impl Read for PtyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buf) {
            Err(e) if e.raw_os_error() == Some(libc::EIO) => Ok(0),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_dans_un_pty() {
        let session = spawn_in_pty(
            Path::new("sh"),
            Path::new("/tmp"),
            ["-c", "echo hello"],
            winsize(24, 80),
        )
        .unwrap();

        let mut out = String::new();
        session.reader().unwrap().read_to_string(&mut out).unwrap();
        assert!(out.contains("hello"));
    }

    #[test]
    fn resize_ne_plante_pas() {
        let mut session = spawn_in_pty(
            Path::new("sleep"),
            Path::new("/tmp"),
            ["5"],
            winsize(24, 80),
        )
        .unwrap();

        session.resize(winsize(40, 120)).unwrap();
        session.kill().unwrap();
    }
}
