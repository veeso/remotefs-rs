//! ## Mock
//!
//! Contains mock for test units

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
use crate::RemoteFs;

// -- mock
pub struct MockRemoteFs;

impl RemoteFs for MockRemoteFs {
    #[allow(unused)]
    fn connect(&mut self) -> crate::RemoteResult<crate::fs::Welcome> {
        Ok(crate::fs::Welcome::default())
    }

    #[allow(unused)]
    fn disconnect(&mut self) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn is_connected(&self) -> bool {
        true
    }

    #[allow(unused)]
    fn pwd(&self) -> crate::RemoteResult<std::path::PathBuf> {
        Ok(std::path::PathBuf::from("/"))
    }

    #[allow(unused)]
    fn change_dir(&self, dir: &std::path::Path) -> crate::RemoteResult<std::path::PathBuf> {
        Ok(dir.to_path_buf())
    }

    #[allow(unused)]
    fn list_dir(&self, path: &std::path::Path) -> crate::RemoteResult<Vec<crate::File>> {
        Ok(vec![])
    }

    #[allow(unused)]
    fn stat(&self, path: &std::path::Path) -> crate::RemoteResult<crate::File> {
        Ok(crate::File {
            path: std::path::PathBuf::from("/foo"),
            metadata: crate::fs::Metadata::default(),
        })
    }

    #[allow(unused)]
    fn setstat(
        &self,
        path: &std::path::Path,
        metadata: crate::fs::Metadata,
    ) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn exists(&self, path: &std::path::Path) -> crate::RemoteResult<bool> {
        Ok(true)
    }

    #[allow(unused)]
    fn remove_file(&self, path: &std::path::Path) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn remove_dir(&self, path: &std::path::Path) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn create_dir(
        &self,
        path: &std::path::Path,
        mode: crate::fs::UnixPex,
    ) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn symlink(&self, path: &std::path::Path, target: &std::path::Path) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn copy(&self, src: &std::path::Path, dest: &std::path::Path) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn mov(&self, src: &std::path::Path, dest: &std::path::Path) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn exec(&self, cmd: &str) -> crate::RemoteResult<(u32, String)> {
        Ok((0, String::default()))
    }

    #[allow(unused)]
    fn append(
        &self,
        path: &std::path::Path,
        metadata: &crate::fs::Metadata,
    ) -> crate::RemoteResult<crate::fs::WriteStream> {
        Err(crate::RemoteError::new(
            crate::RemoteErrorType::UnsupportedFeature,
        ))
    }

    #[allow(unused)]
    fn create(
        &self,
        path: &std::path::Path,
        metadata: &crate::fs::Metadata,
    ) -> crate::RemoteResult<crate::fs::WriteStream> {
        Err(crate::RemoteError::new(
            crate::RemoteErrorType::UnsupportedFeature,
        ))
    }

    #[allow(unused)]
    fn open(&self, path: &std::path::Path) -> crate::RemoteResult<crate::fs::ReadStream> {
        Err(crate::RemoteError::new(
            crate::RemoteErrorType::UnsupportedFeature,
        ))
    }
}
