//! Local portable PTY adapter. Blocking process work stays outside the async reactor.

use std::fmt;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::Error;
use crate::ports::{Launch, Observation, Process, Runtime};
use crate::protocol::{Input, Restore, Size};
use crate::screen::Screen;

/// Local PTY factory using Unix PTYs or Windows `ConPTY`.
#[derive(Debug, Default)]
pub struct LocalRuntime;

impl Runtime for LocalRuntime {
    fn directory(&self, path: &str) -> Result<String, Error> {
        let path = Path::new(path);
        if !path.is_absolute() {
            return Err(Error::Invalid);
        }
        let canonical = path.canonicalize().map_err(|_| Error::Invalid)?;
        if !canonical.is_dir() {
            return Err(Error::Invalid);
        }
        canonical.to_str().map(str::to_owned).ok_or(Error::Invalid)
    }

    fn spawn(&self, launch: &Launch) -> Result<Box<dyn Process>, Error> {
        let size = launch.size.validate()?;
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(|_| Error::Io)?;
        let reader = pair.master.try_clone_reader().map_err(|_| Error::Io)?;
        let writer = pair.master.take_writer().map_err(|_| Error::Io)?;
        let mut command = command(launch);
        command.cwd(&launch.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        // A terminal must never inherit the server's authentication token.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("AIT_SERVER_") {
                command.env_remove(key);
            }
        }
        for (key, value) in &launch.env {
            command.env(key, value);
        }
        let child = pair.slave.spawn_command(command).map_err(|_| Error::Io)?;
        drop(pair.slave);
        let screen = Arc::new(Mutex::new(Screen::new(size)));
        let mut process = LocalProcess {
            master: pair.master,
            child,
            screen: screen.clone(),
            input: None,
            reader: None,
            writer: None,
            reaped: false,
            stopped: false,
        };
        // Construct the owner before starting threads so every failure kills and reaps the child.
        process.reader = Some(
            thread::Builder::new()
                .name("terminal-output".to_owned())
                .spawn(move || read_output(reader, &screen))
                .map_err(|_| Error::Io)?,
        );
        let (input, incoming) = mpsc::sync_channel::<Vec<u8>>(16);
        process.input = Some(input);
        process.writer = Some(
            thread::Builder::new()
                .name("terminal-input".to_owned())
                .spawn(move || write_input(writer, &incoming))
                .map_err(|_| Error::Io)?,
        );
        Ok(Box::new(process))
    }
}

fn command(launch: &Launch) -> CommandBuilder {
    let explicit = launch.command.as_ref();
    let shell =
        std::env::var_os(if cfg!(windows) { "COMSPEC" } else { "SHELL" }).unwrap_or_else(|| {
            if cfg!(windows) {
                "cmd.exe".into()
            } else {
                "/bin/sh".into()
            }
        });
    let mut command = CommandBuilder::new(explicit.map_or(shell.as_os_str(), std::ffi::OsStr::new));
    command.args(&launch.args);
    command
}

struct LocalProcess {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    screen: Arc<Mutex<Screen>>,
    input: Option<mpsc::SyncSender<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    reaped: bool,
    stopped: bool,
}

impl fmt::Debug for LocalProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalProcess")
            .field("pid", &self.child.process_id())
            .finish_non_exhaustive()
    }
}

impl Process for LocalProcess {
    fn title(&self) -> Option<String> {
        lock(&self.screen).title()
    }

    fn exited(&mut self) -> Result<bool, Error> {
        Ok(self.poll_exit()? && lock(&self.screen).drained)
    }

    fn send(&mut self, input: &Input) -> Result<(), Error> {
        match input {
            Input::Resize(resize) => {
                let size = resize.size.validate()?;
                self.master.resize(pty_size(size)).map_err(|_| Error::Io)?;
                lock(&self.screen).resize(size);
                Ok(())
            }
            Input::Input { data } => self.write(data.as_bytes().to_vec()),
            Input::Mouse {
                row,
                col,
                button,
                action,
            } => {
                let bytes = lock(&self.screen).mouse(*row, *col, *button, *action)?;
                self.write(bytes)
            }
        }
    }

    fn observe(
        &mut self,
        revision: Option<u64>,
        restore: Option<&Restore>,
    ) -> Result<Observation, Error> {
        let exited = self.exited()?;
        let mut observation = lock(&self.screen).observe(revision, restore)?;
        observation.exited = exited;
        Ok(observation)
    }

    fn capture(&self) -> Result<Vec<String>, Error> {
        Ok(lock(&self.screen).capture())
    }

    fn kill(&mut self) -> Result<(), Error> {
        if self.stopped {
            return Ok(());
        }
        if !self.reaped {
            self.kill_groups();
        }
        if !self.poll_exit()? {
            self.child.kill().map_err(|_| Error::Io)?;
        }
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.poll_exit()? {
            if Instant::now() >= deadline {
                return Err(Error::Io);
            }
            thread::sleep(Duration::from_millis(5));
        }
        // Readers drain before announcing stream exit. Never block shutdown on a detached descendant.
        for worker in [&mut self.reader, &mut self.writer] {
            while worker.as_ref().is_some_and(|worker| !worker.is_finished())
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(5));
            }
            if worker.as_ref().is_some_and(JoinHandle::is_finished) {
                let _ = worker.take().expect("finished worker is present").join();
            }
        }
        self.stopped = true;
        Ok(())
    }
}

impl LocalProcess {
    fn poll_exit(&mut self) -> Result<bool, Error> {
        if !self.reaped && self.child.try_wait().map_err(|_| Error::Io)?.is_some() {
            // Reap background jobs on first exit observation. Never signal a retained, old PID.
            self.kill_groups();
            self.input.take();
            self.reaped = true;
        }
        Ok(self.reaped)
    }

    fn kill_groups(&self) {
        #[cfg(unix)]
        {
            use nix::sys::signal::{Signal, killpg};
            use nix::unistd::Pid;

            // Job-control shells may put the foreground job in a different process group.
            if let Some(group) = self
                .master
                .process_group_leader()
                .filter(|group| *group > 1)
            {
                let _ = killpg(Pid::from_raw(group), Signal::SIGKILL);
            }
            if let Some(pid) = self
                .child
                .process_id()
                .and_then(|pid| i32::try_from(pid).ok())
            {
                let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
            }
        }
    }

    fn write(&self, bytes: Vec<u8>) -> Result<(), Error> {
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() > 64 * 1024 {
            return Err(Error::Exhausted);
        }
        self.input
            .as_ref()
            .ok_or(Error::NotFound)?
            .try_send(bytes)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => Error::Exhausted,
                mpsc::TrySendError::Disconnected(_) => Error::Io,
            })
    }
}

impl Drop for LocalProcess {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

fn read_output(mut reader: Box<dyn Read + Send>, screen: &Mutex<Screen>) {
    let mut buffer = [0; 16 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => lock(screen).process(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    lock(screen).drained = true;
}

fn write_input(mut writer: Box<dyn Write + Send>, input: &mpsc::Receiver<Vec<u8>>) {
    while let Ok(bytes) = input.recv() {
        if writer
            .write_all(&bytes)
            .and_then(|()| writer.flush())
            .is_err()
        {
            break;
        }
    }
}

fn pty_size(size: Size) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
