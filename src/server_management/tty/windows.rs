use std::{
    ffi::OsStr,
    io::{self, Read, Write},
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::Mutex,
};

use super::Winsize;

/// Évite d'ouvrir une fenêtre de console pour chaque serveur Java.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Session d'un programme lancé avec stdin/stdout/stderr redirigés.
///
/// Windows n'a pas de pty simple d'accès ; les serveurs Minecraft lisent
/// leurs commandes sur stdin et écrivent leurs logs sur stdout, ce qui
/// suffit ici. stdout et stderr sont fusionnés dans le même pipe.
pub struct PtySession {
    pub child: Child,
    stdin: Mutex<ChildStdin>,
    reader: os_pipe::PipeReader,
}

pub fn spawn_in_pty<I, S>(
    program: &Path,
    working_dir: &Path,
    args: I,
    _size: Winsize,
) -> io::Result<PtySession>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let (reader, writer) = os_pipe::pipe()?;
    let stdout = writer.try_clone()?;
    let stderr = writer;

    let mut cmd = Command::new(program);
    cmd.current_dir(working_dir)
        .args(args)
        .env("TERM", "xterm-256color")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "stdin du processus indisponible"))?;

    Ok(PtySession {
        child,
        stdin: Mutex::new(stdin),
        reader,
    })
}

impl PtySession {
    /// Sans pty, la taille du terminal n'a pas de sens.
    pub fn resize(&self, _size: Winsize) -> io::Result<()> {
        Ok(())
    }

    pub fn write_input(&self, data: &[u8]) -> io::Result<()> {
        let mut stdin = self
            .stdin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        stdin.write_all(data)
    }

    pub fn reader(&self) -> io::Result<PtyReader> {
        Ok(PtyReader(self.reader.try_clone()?))
    }

    pub fn kill(&mut self) -> io::Result<()> {
        self.child.kill()
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

pub struct PtyReader(os_pipe::PipeReader);

impl Read for PtyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}
