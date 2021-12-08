//! ## SCP
//!
//! Scp remote fs implementation

/**
 * MIT License
 *
 * remotefs - Copyright (c) 2021 Christian Visintin
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
use super::{commons, SshOpts};
use crate::fs::{
    Metadata, RemoteError, RemoteErrorType, RemoteFs, RemoteResult, UnixPex, UnixPexClass, Welcome,
};
use crate::utils::fmt as fmt_utils;
use crate::utils::parser as parser_utils;
use crate::utils::path as path_utils;
use crate::{Directory, Entry, File};

use regex::Regex;
use std::io::{BufReader, BufWriter, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

// -- export
pub use ssh2::Session as SshSession;

/// SCP "filesystem" client
pub struct ScpFs {
    session: Option<SshSession>,
    wrkdir: PathBuf,
    opts: SshOpts,
}

impl ScpFs {
    /// Creates a new `SftpFs`
    pub fn new(opts: SshOpts) -> Self {
        Self {
            session: None,
            wrkdir: PathBuf::from("/"),
            opts,
        }
    }

    /// Get a reference to current `session` value.
    pub fn session(&mut self) -> Option<&mut SshSession> {
        self.session.as_mut()
    }

    // -- private

    /// Check connection status
    fn check_connection(&self) -> RemoteResult<()> {
        if self.is_connected() {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// ### parse_ls_output
    ///
    /// Parse a line of `ls -l` output and tokenize the output into a `FsEntry`
    fn parse_ls_output(&self, path: &Path, line: &str) -> Result<Entry, ()> {
        // Prepare list regex
        // NOTE: about this damn regex <https://stackoverflow.com/questions/32480890/is-there-a-regex-to-parse-the-values-from-an-ftp-directory-listing>
        lazy_static! {
            static ref LS_RE: Regex = Regex::new(r#"^([\-ld])([\-rwxs]{9})\s+(\d+)\s+(.+)\s+(.+)\s+(\d+)\s+(\w{3}\s+\d{1,2}\s+(?:\d{1,2}:\d{1,2}|\d{4}))\s+(.+)$"#).unwrap();
        }
        trace!("Parsing LS line: '{}'", line);
        // Apply regex to result
        match LS_RE.captures(line) {
            // String matches regex
            Some(metadata) => {
                // NOTE: metadata fmt: (regex, file_type, permissions, link_count, uid, gid, filesize, mtime, filename)
                // Expected 7 + 1 (8) values: + 1 cause regex is repeated at 0
                if metadata.len() < 8 {
                    return Err(());
                }
                // Collect metadata
                // Get if is directory and if is symlink
                let (is_dir, is_symlink): (bool, bool) = match metadata.get(1).unwrap().as_str() {
                    "-" => (false, false),
                    "l" => (false, true),
                    "d" => (true, false),
                    _ => return Err(()), // Ignore special files
                };
                // Check string length (unix pex)
                if metadata.get(2).unwrap().as_str().len() < 9 {
                    return Err(());
                }

                let pex = |range: Range<usize>| {
                    let mut count: u8 = 0;
                    for (i, c) in metadata.get(2).unwrap().as_str()[range].chars().enumerate() {
                        match c {
                            '-' => {}
                            _ => {
                                count += match i {
                                    0 => 4,
                                    1 => 2,
                                    2 => 1,
                                    _ => 0,
                                }
                            }
                        }
                    }
                    count
                };

                // Get unix pex
                let mode = UnixPex::new(
                    UnixPexClass::from(pex(0..3)),
                    UnixPexClass::from(pex(3..6)),
                    UnixPexClass::from(pex(6..9)),
                );

                // Parse mtime and convert to SystemTime
                let mtime: SystemTime = match parser_utils::parse_lstime(
                    metadata.get(7).unwrap().as_str(),
                    "%b %d %Y",
                    "%b %d %H:%M",
                ) {
                    Ok(t) => t,
                    Err(_) => SystemTime::UNIX_EPOCH,
                };
                // Get uid
                let uid: Option<u32> = match metadata.get(4).unwrap().as_str().parse::<u32>() {
                    Ok(uid) => Some(uid),
                    Err(_) => None,
                };
                // Get gid
                let gid: Option<u32> = match metadata.get(5).unwrap().as_str().parse::<u32>() {
                    Ok(gid) => Some(gid),
                    Err(_) => None,
                };
                // Get filesize
                let size = metadata
                    .get(6)
                    .unwrap()
                    .as_str()
                    .parse::<u64>()
                    .unwrap_or(0);
                // Get link and name
                let (file_name, symlink): (String, Option<PathBuf>) = match is_symlink {
                    true => self.get_name_and_link(metadata.get(8).unwrap().as_str()),
                    false => (String::from(metadata.get(8).unwrap().as_str()), None),
                };
                // Sanitize file name
                let file_name = PathBuf::from(&file_name)
                    .file_name()
                    .map(|x| x.to_string_lossy().to_string())
                    .unwrap_or(file_name);
                // Check if file_name is '.' or '..'
                if file_name.as_str() == "." || file_name.as_str() == ".." {
                    debug!("File name is {}; ignoring entry", file_name);
                    return Err(());
                }
                // Re-check if is directory
                let mut abs_path: PathBuf = path.to_path_buf();
                abs_path.push(file_name.as_str());
                // Get extension
                let extension: Option<String> = abs_path
                    .as_path()
                    .extension()
                    .map(|s| String::from(s.to_string_lossy()));
                let metadata = Metadata {
                    atime: SystemTime::UNIX_EPOCH,
                    ctime: SystemTime::UNIX_EPOCH,
                    gid,
                    mode: Some(mode),
                    mtime,
                    size,
                    symlink,
                    uid,
                };
                trace!(
                    "Found entry at {} with metadata {:?}",
                    abs_path.display(),
                    metadata
                );
                // Push to entries
                Ok(match is_dir {
                    true => Entry::Directory(Directory {
                        name: file_name,
                        abs_path,
                        metadata,
                    }),
                    false => Entry::File(File {
                        name: file_name,
                        abs_path,
                        extension,
                        metadata,
                    }),
                })
            }
            None => Err(()),
        }
    }

    /// ### get_name_and_link
    ///
    /// Returns from a `ls -l` command output file name token, the name of the file and the symbolic link (if there is any)
    fn get_name_and_link(&self, token: &str) -> (String, Option<PathBuf>) {
        let tokens: Vec<&str> = token.split(" -> ").collect();
        let filename: String = String::from(*tokens.get(0).unwrap());
        let symlink: Option<PathBuf> = tokens.get(1).map(PathBuf::from);
        (filename, symlink)
    }

    /// Execute setstat command and assert result is 0
    fn assert_stat_command(&mut self, cmd: String) -> RemoteResult<()> {
        match commons::perform_shell_cmd_with_rc(self.session.as_mut().unwrap(), cmd) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::StatFailed)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    /// Returns whether file at `path` is a directory
    fn is_directory(&mut self, path: &Path) -> RemoteResult<bool> {
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("test -d \"{}\"", path.display()),
        ) {
            Ok((0, _)) => Ok(true),
            Ok(_) => Ok(false),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::StatFailed, err)),
        }
    }
}

impl RemoteFs for ScpFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        debug!("Initializing SFTP connection...");
        let mut session = commons::connect(&self.opts)?;
        // Get banner
        let banner: Option<String> = session.banner().map(String::from);
        debug!(
            "Connection established: {}",
            banner.as_deref().unwrap_or("")
        );
        // Get working directory
        debug!("Getting working directory...");
        self.wrkdir = commons::perform_shell_cmd(&mut session, "pwd")
            .map(|x| PathBuf::from(x.as_str().trim()))?;
        // Set session
        self.session = Some(session);
        info!(
            "Connection established; working directory: {}",
            self.wrkdir.display()
        );
        Ok(Welcome::default().banner(banner))
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        debug!("Disconnecting from remote...");
        if let Some(session) = self.session.as_ref() {
            // Disconnect (greet server with 'Mandi' as they do in Friuli)
            match session.disconnect(None, "Mandi!", None) {
                Ok(_) => {
                    // Set session and sftp to none
                    self.session = None;
                    Ok(())
                }
                Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ConnectionError, err)),
            }
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn is_connected(&self) -> bool {
        self.session
            .as_ref()
            .map(|x| x.authenticated())
            .unwrap_or(false)
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        self.check_connection()?;
        Ok(self.wrkdir.clone())
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
        self.check_connection()?;
        let dir = path_utils::absolutize(self.wrkdir.as_path(), dir);
        debug!("Changing working directory to {}", dir.display());
        match commons::perform_shell_cmd(
            self.session.as_mut().unwrap(),
            format!("cd \"{}\"; echo $?; pwd", dir.display()),
        ) {
            Ok(output) => {
                // Trim
                let output: String = String::from(output.as_str().trim());
                // Check if output starts with 0; should be 0{PWD}
                match output.as_str().starts_with('0') {
                    true => {
                        // Set working directory
                        self.wrkdir = PathBuf::from(&output.as_str()[1..].trim());
                        debug!("Changed working directory to {}", self.wrkdir.display());
                        Ok(self.wrkdir.clone())
                    }
                    false => Err(RemoteError::new_ex(
                        // No such file or directory
                        RemoteErrorType::NoSuchFileOrDirectory,
                        format!("\"{}\"", dir.display()),
                    )),
                }
            }
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<Entry>> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!("Getting file entries in {}", path.display());
        match commons::perform_shell_cmd(
            self.session.as_mut().unwrap(),
            format!("unset LANG; ls -la \"{}/\"", path.display()).as_str(),
        ) {
            Ok(output) => {
                // Split output by (\r)\n
                let lines: Vec<&str> = output.as_str().lines().collect();
                let mut entries: Vec<Entry> = Vec::with_capacity(lines.len());
                for line in lines.iter() {
                    // First line must always be ignored
                    // Parse row, if ok push to entries
                    if let Ok(entry) = self.parse_ls_output(path.as_path(), line) {
                        entries.push(entry);
                    }
                }
                debug!(
                    "Found {} out of {} valid file entries",
                    entries.len(),
                    lines.len()
                );
                Ok(entries)
            }
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn stat(&mut self, path: &Path) -> RemoteResult<Entry> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!("Stat {}", path.display());
        // make command; Directories require `-d` option
        let cmd = match self.is_directory(path.as_path())? {
            true => format!("ls -ld \"{}\"", path.display()),
            false => format!("ls -l \"{}\"", path.display()),
        };
        match commons::perform_shell_cmd(self.session.as_mut().unwrap(), cmd.as_str()) {
            Ok(line) => {
                // Parse ls line
                let parent: PathBuf = match path.as_path().parent() {
                    Some(p) => PathBuf::from(p),
                    None => {
                        return Err(RemoteError::new_ex(
                            RemoteErrorType::StatFailed,
                            "Path has no parent",
                        ))
                    }
                };
                match self.parse_ls_output(parent.as_path(), line.as_str().trim()) {
                    Ok(entry) => Ok(entry),
                    Err(_) => Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory)),
                }
            }
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("test -e \"{}\"", path.display()),
        ) {
            Ok((0, _)) => Ok(true),
            Ok(_) => Ok(false),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::StatFailed, err)),
        }
    }

    fn setstat(&mut self, path: &Path, metadata: Metadata) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!("Setting attributes for {}", path.display());
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        // set mode with chmod
        if let Some(mode) = metadata.mode {
            self.assert_stat_command(format!(
                "chmod {:o} \"{}\"",
                u32::from(mode),
                path.display()
            ))?;
        }
        // set times
        self.assert_stat_command(format!(
            "touch -a -t {} \"{}\"",
            fmt_utils::fmt_time_utc(metadata.atime, "%Y%m%d%H%M.%S"),
            path.display()
        ))?;
        self.assert_stat_command(format!(
            "touch -m -t {} \"{}\"",
            fmt_utils::fmt_time_utc(metadata.mtime, "%Y%m%d%H%M.%S"),
            path.display()
        ))
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        debug!("Removing file {}", path.display());
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("rm -f \"{}\"", path.display()),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        debug!("Removing directory {}", path.display());
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("rmdir \"{}\"", path.display()),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::DirectoryNotEmpty)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn remove_dir_all(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        debug!("Removing directory {} recursively", path.display());
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("rm -rf \"{}\"", path.display()),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn create_dir(&mut self, path: &Path, mode: UnixPex) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        if self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::DirectoryAlreadyExists));
        }
        let mode = format!("{:o}", u32::from(mode));
        debug!(
            "Creating directory at {} with mode {}",
            path.display(),
            mode
        );
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("mkdir -m {} \"{}\"", mode, path.display()),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::FileCreateDenied)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn symlink(&mut self, path: &Path, target: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!(
            "Creating a symlink at {} pointing at {}",
            path.display(),
            target.display()
        );
        if !self.exists(target).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        if self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::FileCreateDenied));
        }
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("ln -s \"{}\" \"{}\"", target.display(), path.display()),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::FileCreateDenied)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn copy(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let src = path_utils::absolutize(self.wrkdir.as_path(), src);
        // check if file exists
        if !self.exists(src.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        let dest = path_utils::absolutize(self.wrkdir.as_path(), dest);
        debug!("Copying {} to {}", src.display(), dest.display());
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("cp -rf \"{}\" \"{}\"", src.display(), dest.display()).as_str(),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new_ex(
                // Could not copy file
                RemoteErrorType::FileCreateDenied,
                format!("\"{}\"", dest.display()),
            )),
            Err(err) => Err(RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                err.to_string(),
            )),
        }
    }

    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let src = path_utils::absolutize(self.wrkdir.as_path(), src);
        // check if file exists
        if !self.exists(src.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        let dest = path_utils::absolutize(self.wrkdir.as_path(), dest);
        debug!("Moving {} to {}", src.display(), dest.display());
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("mv -f \"{}\" \"{}\"", src.display(), dest.display()).as_str(),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new_ex(
                // Could not copy file
                RemoteErrorType::FileCreateDenied,
                format!("\"{}\"", dest.display()),
            )),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn exec(&mut self, cmd: &str) -> RemoteResult<(u32, String)> {
        self.check_connection()?;
        debug!(r#"Executing command "{}""#, cmd);
        commons::perform_shell_cmd_at_with_rc(
            self.session.as_mut().unwrap(),
            cmd,
            self.wrkdir.as_path(),
        )
    }

    fn append(&mut self, _path: &Path, _metadata: &Metadata) -> RemoteResult<Box<dyn Write>> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn create(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<Box<dyn Write>> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!("Creating file {}", path.display());
        // blocking channel
        self.session.as_mut().unwrap().set_blocking(true);
        trace!("blocked channel");
        let mode = metadata.mode.map(u32::from).unwrap_or(0o644) as i32;
        let atime = metadata
            .atime
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let mtime = metadata
            .mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .unwrap_or(Duration::ZERO)
            .as_secs();
        trace!(
            "Creating file with mode {:o}, atime: {}, mtime: {}",
            mode,
            atime,
            mtime
        );
        match self.session.as_mut().unwrap().scp_send(
            path.as_path(),
            mode,
            metadata.size,
            Some((mtime, atime)),
        ) {
            Ok(channel) => Ok(Box::new(BufWriter::with_capacity(65536, channel))),
            Err(err) => {
                error!("Failed to create file: {}", err);
                Err(RemoteError::new_ex(RemoteErrorType::FileCreateDenied, err))
            }
        }
    }

    fn open(&mut self, path: &Path) -> RemoteResult<Box<dyn Read>> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!("Opening file {} for read", path.display());
        // check if file exists
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        self.session.as_mut().unwrap().set_blocking(true);
        trace!("blocked channel");
        match self.session.as_mut().unwrap().scp_recv(path.as_path()) {
            Ok((channel, _)) => Ok(Box::new(BufReader::with_capacity(65536, channel))),
            Err(err) => {
                error!("Failed to open file: {}", err);
                Err(RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, err))
            }
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use crate::mock::fs as fs_mock;
    use crate::mock::ssh as ssh_mock;

    use pretty_assertions::assert_eq;
    use std::fs::File as StdFile;

    #[test]
    fn should_init_scp_fs() {
        let client = ScpFs::new(SshOpts::new("localhost"));
        assert!(client.session.is_none());
        assert_eq!(client.is_connected(), false);
    }

    #[test]
    fn should_fail_connection_to_bad_server() {
        let mut client = ScpFs::new(SshOpts::new("mybad.verybad.server"));
        assert!(client.connect().is_err());
    }

    #[test]
    #[cfg(feature = "with-containers")]
    fn should_operate_on_scp_file_system() {
        crate::mock::logger();
        let config_file = ssh_mock::create_ssh_config();
        let mut client = ScpFs::new(
            SshOpts::new("scp")
                .key_storage(Box::new(ssh_mock::MockSshKeyStorage::default()))
                .config_file(config_file.path()),
        );
        // Sample file
        let (entry, file) = fs_mock::create_sample_file_entry();
        // Connect
        assert!(client.connect().is_ok());
        // Check session
        assert!(client.session.is_some());
        assert_eq!(client.wrkdir, PathBuf::from("/config"));
        assert_eq!(client.is_connected(), true);
        // Pwd
        assert_eq!(client.wrkdir.clone(), client.pwd().ok().unwrap());
        // Cleanup
        assert!(client.change_dir(Path::new("/")).is_ok());
        let _ = client.remove_dir_all(Path::new("/tmp/omar"));
        let _ = client.remove_dir_all(Path::new("/tmp/uploads"));
        // Stat
        let stat = client
            .stat(Path::new("/config/sshd.pid"))
            .ok()
            .unwrap()
            .unwrap_file();
        assert_eq!(stat.name.as_str(), "sshd.pid");
        let stat = client
            .stat(Path::new("/config/"))
            .ok()
            .unwrap()
            .unwrap_dir();
        assert_eq!(stat.name.as_str(), "config");
        // Stat (err)
        assert!(client.stat(Path::new("/config/5t0ca220.log")).is_err());
        // List dir (dir has 4 (one is hidden :D) entries)
        assert!(client.list_dir(&Path::new("/config")).unwrap().len() >= 4);
        // Make directory
        assert!(client
            .create_dir(Path::new("/tmp/omar"), UnixPex::from(0o775))
            .is_ok());
        // Remake directory (should report already exists)
        assert_eq!(
            client
                .create_dir(Path::new("/tmp/omar"), UnixPex::from(0o775))
                .err()
                .unwrap()
                .kind,
            RemoteErrorType::DirectoryAlreadyExists
        );
        // Make directory (err)
        assert!(client
            .create_dir(Path::new("/root/aaaaa/pommlar"), UnixPex::from(0o775))
            .is_err());
        // Change directory
        assert!(client.change_dir(Path::new("/tmp/omar")).is_ok());
        // Change directory (err)
        assert!(client.change_dir(Path::new("/tmp/oooo/aaaa/eee")).is_err());
        // Copy (not supported)
        assert!(client
            .copy(entry.abs_path.as_path(), Path::new("/"))
            .is_err());
        // Exec
        assert_eq!(client.exec("echo 5").ok().unwrap(), (0, "5\n".to_string()));
        // Upload 2 files
        let mut writable = client
            .create(Path::new("omar.txt"), &entry.metadata)
            .ok()
            .unwrap();
        fs_mock::write_file(&file, &mut writable);
        assert!(client.on_written(writable).is_ok());
        let mut writable = client
            .create(Path::new("README.md"), &entry.metadata)
            .ok()
            .unwrap();
        fs_mock::write_file(&file, &mut writable);
        assert!(client.on_written(writable).is_ok());
        // Set stat
        let metadata = client
            .stat(Path::new("README.md"))
            .ok()
            .unwrap()
            .metadata()
            .clone();
        assert!(client.setstat(Path::new("README.md"), metadata).is_ok());
        // Upload file without stream
        let reader = Box::new(StdFile::open(entry.abs_path.as_path()).ok().unwrap());
        assert!(client
            .create_file(Path::new("README2.md"), &entry.metadata, reader)
            .is_ok());
        // Upload file (err)
        assert!(client
            .create(Path::new("/ommlar/omarone"), &entry.metadata)
            .is_err());
        // List dir
        let list = client.list_dir(Path::new("/tmp/omar")).ok().unwrap();
        assert_eq!(list.len(), 3);
        // Find
        assert_eq!(client.find("*.txt").ok().unwrap().len(), 1);
        assert_eq!(client.find("*.md").ok().unwrap().len(), 2);
        assert_eq!(client.find("*.jpeg").ok().unwrap().len(), 0);
        // Rename
        assert!(client
            .create_dir(Path::new("/tmp/uploads"), UnixPex::from(0o775))
            .is_ok());
        assert!(client
            .mov(
                list.get(0).unwrap().path(),
                Path::new("/tmp/uploads/README.txt")
            )
            .is_ok());
        // Rename (err)
        assert!(client
            .mov(list.get(0).unwrap().path(), Path::new("OMARONE"))
            .is_err());
        // Symlink
        let _ = client.exec("rm -f /tmp/README.txt");
        assert!(client
            .symlink(
                Path::new("/tmp/README.txt"),
                Path::new("/tmp/uploads/README.txt")
            )
            .is_ok());
        // Symlink (err)
        assert!(client
            .symlink(
                Path::new("/tmp/uploads/PIPPO.txt"),
                Path::new("/tmp/omarone.log")
            )
            .is_err());
        let dummy = Entry::File(File {
            name: String::from("cucumber.txt"),
            abs_path: PathBuf::from("/cucumber.txt"),
            extension: None,
            metadata: Metadata::default(),
        });
        assert!(client.mov(&dummy.path(), Path::new("/a/b/c")).is_err());
        // Remove
        assert!(client.remove_dir_all(list.get(1).unwrap().path()).is_ok());
        assert!(client.remove_dir_all(list.get(1).unwrap().path()).is_err());
        // Receive file
        let mut writable = client
            .create(Path::new("/tmp/uploads/README.txt"), &entry.metadata)
            .ok()
            .unwrap();
        fs_mock::write_file(&file, &mut writable);
        assert!(client.on_written(writable).is_ok());
        let file = client
            .list_dir(Path::new("/tmp/uploads"))
            .ok()
            .unwrap()
            .get(0)
            .unwrap()
            .clone()
            .unwrap_file();
        let mut readable = client.open(file.abs_path.as_path()).ok().unwrap();
        let mut data: Vec<u8> = vec![0; 1024];
        assert!(readable.read(&mut data).is_ok());
        assert!(client.on_read(readable).is_ok());
        let mut dest_file = fs_mock::create_sample_file();
        // Receive file wno stream
        assert!(client
            .open_file(file.abs_path.as_path(), &mut dest_file)
            .is_ok());
        // Receive file (err)
        assert!(client.open(entry.abs_path.as_path()).is_err());
        // Cleanup
        assert!(client.change_dir(Path::new("/")).is_ok());
        assert!(client.remove_dir_all(Path::new("/tmp/omar")).is_ok());
        assert!(client.remove_dir_all(Path::new("/tmp/uploads")).is_ok());
        // Disconnect
        assert!(client.disconnect().is_ok());
        assert_eq!(client.is_connected(), false);
    }

    #[test]
    fn should_get_name_and_link() {
        let client = ScpFs::new(SshOpts::new("localhost"));
        assert_eq!(
            client.get_name_and_link("Cargo.toml"),
            (String::from("Cargo.toml"), None)
        );
        assert_eq!(
            client.get_name_and_link("Cargo -> Cargo.toml"),
            (String::from("Cargo"), Some(PathBuf::from("Cargo.toml")))
        );
    }

    #[test]
    fn should_parse_file_ls_output() {
        let client = ScpFs::new(SshOpts::new("localhost"));
        // File
        let entry = client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "-rw-r--r-- 1 root root  2056 giu 13 21:11 /tmp/Cargo.toml",
            )
            .ok()
            .unwrap()
            .unwrap_file();
        assert_eq!(entry.name.as_str(), "Cargo.toml");
        assert_eq!(entry.abs_path, PathBuf::from("/tmp/Cargo.toml"));
        assert_eq!(u32::from(entry.metadata.mode.unwrap()), 0o644_u32);
        assert_eq!(entry.metadata.size, 2056);
        assert_eq!(entry.extension.unwrap().as_str(), "toml");
        assert!(entry.metadata.symlink.is_none());
        // File (year)
        let entry = client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "-rw-rw-rw- 1 root root  3368 nov  7  2020 CODE_OF_CONDUCT.md",
            )
            .ok()
            .unwrap()
            .unwrap_file();
        assert_eq!(entry.name.as_str(), "CODE_OF_CONDUCT.md");
        assert_eq!(entry.abs_path, PathBuf::from("/tmp/CODE_OF_CONDUCT.md"));
        assert_eq!(u32::from(entry.metadata.mode.unwrap()), 0o666_u32);
        assert_eq!(entry.metadata.size, 3368);
        assert_eq!(entry.extension.unwrap().as_str(), "md");
        assert!(entry.metadata.symlink.is_none());
    }

    #[test]
    fn should_parse_directory_from_ls_output() {
        let client = ScpFs::new(SshOpts::new("localhost"));
        // Directory
        let entry = client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "drwxr-xr-x 1 root root   512 giu 13 21:11 docs",
            )
            .ok()
            .unwrap()
            .unwrap_dir();
        assert_eq!(entry.name.as_str(), "docs");
        assert_eq!(entry.abs_path, PathBuf::from("/tmp/docs"));
        assert_eq!(u32::from(entry.metadata.mode.unwrap()), 0o755_u32);
        assert!(entry.metadata.symlink.is_none());
        // Short metadata
        assert!(client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "drwxr-xr-x 1 root root   512 giu 13 21:11",
            )
            .is_err());
        // Special file
        assert!(client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "crwxr-xr-x 1 root root   512 giu 13 21:11 ttyS1",
            )
            .is_err());
        // Bad pex
        assert!(client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "-rwxr-xr 1 root root   512 giu 13 21:11 ttyS1",
            )
            .is_err());
    }

    #[test]
    fn should_parse_symlink_from_ls_output() {
        let client = ScpFs::new(SshOpts::new("localhost"));
        // File
        let entry = client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "lrw-r--r-- 1 root root  2056 giu 13 21:11 Cargo.toml -> Cargo.prod.toml",
            )
            .ok()
            .unwrap()
            .unwrap_file();
        assert_eq!(entry.name.as_str(), "Cargo.toml");
        assert_eq!(entry.abs_path, PathBuf::from("/tmp/Cargo.toml"));
        assert_eq!(u32::from(entry.metadata.mode.unwrap()), 0o644_u32);
        assert_eq!(entry.metadata.size, 2056);
        assert_eq!(entry.extension.unwrap().as_str(), "toml");
        assert_eq!(
            entry.metadata.symlink.as_deref().unwrap(),
            Path::new("Cargo.prod.toml")
        );
    }

    #[test]
    fn should_return_errors_on_uninitialized_client() {
        let mut client = ScpFs::new(SshOpts::new("localhost"));
        assert!(client.change_dir(Path::new("/tmp")).is_err());
        assert!(client
            .copy(Path::new("/nowhere"), PathBuf::from("/culonia").as_path())
            .is_err());
        assert!(client.exec("echo 5").is_err());
        assert!(client.disconnect().is_err());
        assert!(client.list_dir(Path::new("/tmp")).is_err());
        assert!(client
            .create_dir(Path::new("/tmp"), UnixPex::from(0o755))
            .is_err());
        assert!(client.symlink(Path::new("/a"), Path::new("/b")).is_err());
        assert!(client.pwd().is_err());
        assert!(client.remove_dir_all(Path::new("/nowhere")).is_err());
        assert!(client
            .mov(Path::new("/nowhere"), Path::new("/culonia"))
            .is_err());
        assert!(client.stat(Path::new("/tmp")).is_err());
        assert!(client
            .setstat(Path::new("/tmp"), Metadata::default())
            .is_err());
        assert!(client.open(Path::new("/tmp/pippo.txt")).is_err());
        assert!(client
            .create(Path::new("/tmp/pippo.txt"), &Metadata::default())
            .is_err());
        assert!(client
            .append(Path::new("/tmp/pippo.txt"), &Metadata::default())
            .is_err());
    }
}
