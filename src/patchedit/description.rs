use std::io::Write;

use chrono::DateTime;

use crate::error::Error;
use crate::patchname::PatchName;
use crate::signature::{self, TimeExtended};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct DiffBuffer(pub(crate) Vec<u8>);

impl AsRef<[u8]> for DiffBuffer {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Default)]
pub(crate) struct PatchDescription {
    pub patchname: Option<PatchName>,
    pub author: Option<git2::Signature<'static>>,
    pub message: String,
    pub instruction: Option<&'static str>,
    pub diff_instruction: Option<&'static str>,
    pub diff: Option<DiffBuffer>,
}

impl PatchDescription {
    pub(crate) fn write<S: Write>(&self, stream: &mut S) -> Result<(), Error> {
        let patchname = if let Some(patchname) = &self.patchname {
            patchname.as_ref()
        } else {
            ""
        };
        writeln!(stream, "Patch:  {patchname}")?;
        if let Some(author) = self.author.as_ref() {
            let authdate = author.datetime().format("%Y-%m-%d %H:%M:%S %z");
            write!(stream, "Author: ")?;
            stream.write_all(author.name_bytes())?;
            write!(stream, " <")?;
            stream.write_all(author.email_bytes())?;
            write!(stream, ">\nDate:   {authdate}\n",)?;
        } else {
            writeln!(stream, "Author: ")?;
            writeln!(stream, "Date:   ")?;
        }
        let message = self.message.trim_end_matches('\n');
        write!(stream, "\n{message}\n")?;
        if let Some(instruction) = self.instruction {
            write!(stream, "\n{}", instruction)?;
        } else {
            writeln!(stream)?;
        }
        if let Some(diff) = self.diff.as_ref() {
            if let Some(diff_instruction) = self.diff_instruction {
                write!(stream, "{}", diff_instruction)?;
            }
            stream.write_all(b"---\n")?;
            stream.write_all(diff.as_ref())?;
        }
        Ok(())
    }
}

impl TryFrom<&[u8]> for PatchDescription {
    type Error = Error;

    fn try_from(buf: &[u8]) -> Result<Self, Self::Error> {
        let mut raw_patchname: Option<String> = None;
        let mut raw_author: Option<String> = None;
        let mut raw_authdate: Option<String> = None;
        let mut consume_diff: bool = false;
        let mut consuming_message: bool = false;
        let mut consecutive_empty: usize = 0;
        let mut message = String::new();
        let mut pos: usize = 0;

        for (line_num, line) in buf.split_inclusive(|&b| b == b'\n').enumerate() {
            pos += line.len();
            if line.starts_with(b"#") {
                continue;
            }

            // Every line before the diff must be valid utf8
            let line = std::str::from_utf8(line).map_err(|_| Error::NonUtf8PatchDescription)?;
            let trimmed = line.trim_end();

            if trimmed == "---" {
                consume_diff = true;
                break;
            }

            if consuming_message {
                if trimmed.is_empty() {
                    if consecutive_empty == 0 {
                        message.push('\n');
                    }
                    consecutive_empty += 1;
                } else {
                    consecutive_empty = 0;
                    message.push_str(trimmed);
                    message.push('\n');
                }
            } else {
                if let Some((key, value)) = trimmed.split_once(':') {
                    if line_num == 0 && key == "Patch" {
                        raw_patchname = Some(value.trim().to_string());
                        continue;
                    } else if line_num == 1 && key == "Author" {
                        raw_author = Some(value.trim().to_string());
                        continue;
                    } else if line_num == 2 && key == "Date" {
                        raw_authdate = Some(value.trim().to_string());
                        continue;
                    }
                }

                // Swallow a single blank line after the headers
                if !trimmed.is_empty() {
                    message.push_str(trimmed);
                    message.push('\n');
                } else {
                    consecutive_empty += 1;
                }
                consuming_message = true;
            }
        }

        if raw_patchname.is_none()
            && raw_author.is_none()
            && raw_authdate.is_none()
            && message.trim().is_empty()
        {
            return Err(Error::Generic(
                "Aborting edit due to empty patch description".to_string(),
            ));
        }

        let patchname = if let Some(patchname) = raw_patchname {
            if !patchname.is_empty() {
                Some(patchname.parse::<PatchName>()?)
            } else {
                None
            }
        } else {
            None
        };

        let author = if let Some(author) = raw_author {
            let (name, email) = signature::parse_name_email(&author)?;
            if let Some(date_str) = raw_authdate {
                let dt = DateTime::parse_from_str(&date_str, "%Y-%m-%d %H:%M:%S %z")
                    .map_err(|_| Error::InvalidDate(date_str, "patch description".to_string()))?;
                let when = git2::Time::new(dt.timestamp(), dt.offset().local_minus_utc() / 60);
                Some(git2::Signature::new(name, email, &when)?)
            } else {
                Some(git2::Signature::now(name, email)?)
            }
        } else {
            None
        };

        if message.ends_with("\n\n") {
            message.pop();
        }

        if message.trim().is_empty() {
            message.clear();
        }

        let diff = if consume_diff {
            let diff_slice = &buf[pos..];
            if diff_slice.iter().all(|b| b.is_ascii_whitespace()) {
                None
            } else {
                Some(DiffBuffer(diff_slice.to_vec()))
            }
        } else {
            None
        };

        let instruction = None;
        let diff_instruction = None;

        Ok(Self {
            patchname,
            author,
            message,
            instruction,
            diff_instruction,
            diff,
        })
    }
}

fn _find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if haystack.len() >= needle.len() {
        for i in 0..haystack.len() - needle.len() + 1 {
            if haystack[i..i + needle.len()] == needle[..] {
                return Some(i);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compare_patch_descs(pd0: &PatchDescription, pd1: &PatchDescription) {
        assert_eq!(pd0.patchname, pd1.patchname);
        if let (Some(author0), Some(author1)) = (pd0.author.as_ref(), pd1.author.as_ref()) {
            assert_eq!(author0.name(), author1.name());
            assert_eq!(author0.email(), author1.email());
            assert_eq!(author0.when().seconds(), author1.when().seconds());
            assert_eq!(
                author0.when().offset_minutes(),
                author1.when().offset_minutes()
            );
        } else {
            assert!(pd0.author.is_none() && pd1.author.is_none());
        }
        assert_eq!(pd0.message, pd1.message);
        if let (Some(diff0), Some(diff1)) = (&pd0.diff, &pd1.diff) {
            assert_eq!(
                std::str::from_utf8(diff0.0.as_slice()).unwrap(),
                std::str::from_utf8(diff1.0.as_slice()).unwrap(),
            )
        } else {
            assert!(
                pd0.diff == pd1.diff,
                "diffs differ pd0 is {} pd1 is {}",
                if pd0.diff.is_some() { "Some" } else { "None" },
                if pd1.diff.is_some() { "Some" } else { "None" },
            );
        }
    }

    #[test]
    fn round_trip_no_message_no_diff() {
        let patch_desc = PatchDescription {
            patchname: Some("patch".parse::<PatchName>().unwrap()),
            author: Some(
                git2::Signature::new(
                    "The Author",
                    "author@example.com",
                    &git2::Time::new(987654321, -60),
                )
                .unwrap(),
            ),
            message: "".to_string(),
            instruction: Some("# Instruction\n"),
            diff_instruction: None,
            diff: None,
        };

        let mut buf: Vec<u8> = vec![];
        patch_desc.write(&mut buf).unwrap();

        assert_eq!(
            std::str::from_utf8(buf.as_slice()).unwrap(),
            "Patch:  patch\n\
             Author: The Author <author@example.com>\n\
             Date:   2001-04-19 03:25:21 -0100\n\
             \n\
             \n\
             \n\
             # Instruction\n",
        );

        let new_pd = PatchDescription::try_from(buf.as_slice()).unwrap();

        compare_patch_descs(&new_pd, &patch_desc);
    }

    #[test]
    fn round_trip_one_line_message() {
        let patch_desc = PatchDescription {
            patchname: Some("patch".parse::<PatchName>().unwrap()),
            author: Some(
                git2::Signature::new(
                    "The Author",
                    "author@example.com",
                    &git2::Time::new(987654321, 360),
                )
                .unwrap(),
            ),
            message: "Subject\n".to_string(),
            instruction: Some("# Instruction\n"),
            diff_instruction: None,
            diff: None,
        };

        let mut buf: Vec<u8> = vec![];
        patch_desc.write(&mut buf).unwrap();

        assert_eq!(
            std::str::from_utf8(buf.as_slice()).unwrap(),
            "Patch:  patch\n\
             Author: The Author <author@example.com>\n\
             Date:   2001-04-19 10:25:21 +0600\n\
             \n\
             Subject\n\
             \n\
             # Instruction\n",
        );

        let new_pd = PatchDescription::try_from(buf.as_slice()).unwrap();

        compare_patch_descs(&new_pd, &patch_desc);
    }

    #[test]
    fn round_trip_multi_line_message() {
        let patch_desc = PatchDescription {
            patchname: Some("patch".parse::<PatchName>().unwrap()),
            author: Some(
                git2::Signature::new(
                    "The Author",
                    "author@example.com",
                    &git2::Time::new(987654321, 360),
                )
                .unwrap(),
            ),
            message: "Subject\n\
                      \n\
                      Body of message.\n\
                      More body of message.\n\
                      \n\
                      With-a-trailer: yes\n\
                      "
            .to_string(),
            instruction: Some("# Instruction\n"),
            diff_instruction: None,
            diff: None,
        };

        let mut buf: Vec<u8> = vec![];
        patch_desc.write(&mut buf).unwrap();

        assert_eq!(
            std::str::from_utf8(buf.as_slice()).unwrap(),
            "Patch:  patch\n\
             Author: The Author <author@example.com>\n\
             Date:   2001-04-19 10:25:21 +0600\n\
             \n\
             Subject\n\
             \n\
             Body of message.\n\
             More body of message.\n\
             \n\
             With-a-trailer: yes\n\
             \n\
             # Instruction\n",
        );

        let new_pd = PatchDescription::try_from(buf.as_slice()).unwrap();

        compare_patch_descs(&new_pd, &patch_desc);
    }

    #[test]
    fn with_diff() {
        let pd = PatchDescription {
            patchname: Some("patch".parse::<PatchName>().unwrap()),
            author: Some(
                git2::Signature::new(
                    "The Author",
                    "author@example.com",
                    &git2::Time::new(987654321, 360),
                )
                .unwrap(),
            ),
            message: "Subject\n".to_string(),
            instruction: Some("# Instruction\n"),
            diff_instruction: Some("# Diff instruction\n"),
            diff: Some(DiffBuffer(
                b"\n\
                  Some stuff before first diff --git\n\
                  \n\
                  diff --git a/foo.txt b/foo.txt\n\
                  index ce01362..a21e91b 100644\n\
                  --- a/foo.txt\n\
                  +++ b/foo.txt\n\
                  @@ -1 +1 @@\n\
                  -hello\n\
                  +goodbye\n\
                  \\ No newline at end of file\n"
                    .to_vec(),
            )),
        };

        let mut buf: Vec<u8> = vec![];
        pd.write(&mut buf).unwrap();

        assert_eq!(
            std::str::from_utf8(buf.as_slice()).unwrap(),
            "Patch:  patch\n\
             Author: The Author <author@example.com>\n\
             Date:   2001-04-19 10:25:21 +0600\n\
             \n\
             Subject\n\
             \n\
             # Instruction\n\
             # Diff instruction\n\
             ---\n\
             \n\
             Some stuff before first diff --git\n\
             \n\
             diff --git a/foo.txt b/foo.txt\n\
             index ce01362..a21e91b 100644\n\
             --- a/foo.txt\n\
             +++ b/foo.txt\n\
             @@ -1 +1 @@\n\
             -hello\n\
             +goodbye\n\
             \\ No newline at end of file\n",
        );

        let new_pd = PatchDescription::try_from(buf.as_slice()).unwrap();

        compare_patch_descs(&new_pd, &pd);
    }

    #[test]
    fn with_extra_comments() {
        let patch_desc = PatchDescription {
            patchname: Some("patch".parse::<PatchName>().unwrap()),
            author: Some(
                git2::Signature::new(
                    "The Author",
                    "author@example.com",
                    &git2::Time::new(987654321, 360),
                )
                .unwrap(),
            ),
            message: "Subject\n\
                      \n\
                      Body of message.\n   # Indented: not a comment.\n\
                      \n\
                      With-a-trailer: yes\n\
                      "
            .to_string(),
            instruction: Some("# Instruction\n"),
            diff_instruction: None,
            diff: None,
        };

        let mut buf: Vec<u8> = vec![];
        patch_desc.write(&mut buf).unwrap();

        assert_eq!(
            std::str::from_utf8(buf.as_slice()).unwrap(),
            "Patch:  patch\n\
             Author: The Author <author@example.com>\n\
             Date:   2001-04-19 10:25:21 +0600\n\
             \n\
             Subject\n\
             \n\
             Body of message.\n   # Indented: not a comment.\n\
             \n\
             With-a-trailer: yes\n\
             \n\
             # Instruction\n",
        );

        let updated = b"Patch:  patch\n\
                        Author: The Author <author@example.com>\n\
                        Date:   2001-04-19 10:25:21 +0600\n\
                        # Next line must be blank.\n\
                        \n\
                        # Subject is below\n\
                        Subject\n\
                        \n\
                        Body of message.\n   # Indented: not a comment.\n\
                        \n\
                        # Trailer is below\n\
                        With-a-trailer: yes\n\
                        \n\
                        # Instruction\n";

        let new_pd = PatchDescription::try_from(updated.as_slice()).unwrap();

        compare_patch_descs(&new_pd, &patch_desc);
    }

    #[test]
    fn missing_patch_header() {
        let description = b"\
        # Patch:  patch\n\
        Author: The Author <author@example.com>\n\
        Date:   2001-04-19 10:25:21 +0600\n\
        \n\
        Subject\n\
        \n\
        # Instruction\n";

        let pd = PatchDescription::try_from(description.as_slice()).unwrap();

        let expected = PatchDescription {
            patchname: None,
            author: Some(
                git2::Signature::new(
                    "The Author",
                    "author@example.com",
                    &git2::Time::new(987654321, 360),
                )
                .unwrap(),
            ),
            message: "Subject\n".to_string(),
            instruction: Some("# Instruction\n"),
            diff_instruction: None,
            diff: None,
        };

        compare_patch_descs(&expected, &pd);
    }

    #[test]
    fn missing_date_header() {
        let description = b"\
        Patch:  patch\n\
        Author: The Author <author@example.com>\n\
        # Date:   2001-04-19 10:25:21 +0600\n\
        \n\
        Subject\n\
        \n\
        # Instruction\n";

        let pd = PatchDescription::try_from(description.as_slice()).unwrap();

        let commented_time = git2::Time::new(987654321, 360);
        assert!(pd.author.is_some());
        // Author date should be "now" if Author is present and Date is missing.
        // We just check that the commented-out time is not used.
        assert_ne!(pd.author.as_ref().unwrap().when(), commented_time);

        let expected = PatchDescription {
            patchname: Some("patch".parse::<PatchName>().unwrap()),
            author: Some(
                git2::Signature::new(
                    "The Author",
                    "author@example.com",
                    &pd.author.as_ref().unwrap().when(),
                )
                .unwrap(),
            ),
            message: "Subject\n".to_string(),
            instruction: None,
            diff_instruction: None,
            diff: None,
        };

        compare_patch_descs(&expected, &pd);
    }

    #[test]
    fn extra_header() {
        let description = b"\
        Patch:  patch\n\
        Extra:  nope\n\
        Author: The Author <author@example.com>\n\
        Date:   2001-04-19 10:25:21 +0600\n\
        \n\
        Subject\n\
        \n\
        # Instruction\n";

        let pd = PatchDescription::try_from(description.as_slice()).unwrap();

        let expected = PatchDescription {
            patchname: Some("patch".parse::<PatchName>().unwrap()),
            author: None,
            message: "Extra:  nope\n\
                      Author: The Author <author@example.com>\n\
                      Date:   2001-04-19 10:25:21 +0600\n\
                      \n\
                      Subject\n"
                .to_string(),
            instruction: None,
            diff_instruction: None,
            diff: None,
        };

        compare_patch_descs(&expected, &pd);
    }

    #[test]
    fn invalid_date() {
        let description = b"\
        Patch:  patch\n\
        Author: The Author <author@example.com>\n\
        Date:   2001/04/19 10:25:21 +0600\n\
        \n\
        Subject\n\
        \n\
        # Instruction\n";

        assert!(PatchDescription::try_from(description.as_slice()).is_err());
    }

    #[test]
    fn no_blank_before_message() {
        let description = b"\
        Patch:  patch\n\
        Author: The Author <author@example.com>\n\
        Date:   2001-04-19 10:25:21 +0600\n\
        Subject\n\
        \n\
        # Instruction\n";

        let pd = PatchDescription::try_from(description.as_slice()).unwrap();
        assert_eq!(pd.message, "Subject\n");
    }

    #[test]
    fn extra_blanks_before_message() {
        let description = b"\
        Patch:  patch\n\
        Author: The Author <author@example.com>\n\
        Date:   2001-04-19 10:25:21 +0600\n\
        \n\
        \n\
        \n\
        Subject\n\
        \n\
        # Instruction\n";

        let pd = PatchDescription::try_from(description.as_slice()).unwrap();

        assert_eq!(pd.message, "Subject\n");
    }

    #[test]
    fn no_blank_before_end() {
        let description = b"\
        Patch:  patch\n\
        Author: The Author <author@example.com>\n\
        Date:   2001-04-19 10:25:21 +0600\n\
        \n\
        Subject\n\
        # Instruction\n";

        let pd = PatchDescription::try_from(description.as_slice()).unwrap();

        assert_eq!(pd.message, "Subject\n");
    }

    #[test]
    fn no_eol_message() {
        let description = b"\
        Patch:  patch\n\
        Author: The Author <author@example.com>\n\
        Date:   2001-04-19 10:25:21 +0600\n\
        \n\
        Subject";

        let pd = PatchDescription::try_from(description.as_slice()).unwrap();

        assert_eq!(pd.message, "Subject\n");
    }

    #[test]
    fn extra_blanks_in_message() {
        let description = b"\
        Patch:  patch\n\
        Author: The Author <author@example.com>\n\
        Date:   2001-04-19 10:25:21 +0600\n\
        \n\
        Subject\n\
        \n\
        \n\
        body\n\
        \n\
        \n  \
        \n\
        more body\n\
        \n\t\
        \n\
        # Instruction\n\
        \n";

        let pd = PatchDescription::try_from(description.as_slice()).unwrap();

        assert_eq!(
            pd.message,
            "Subject\n\
             \n\
             body\n\
             \n\
             more body\n"
        );
    }

    #[test]
    fn empty_diff() {
        for description in [
            b"---".as_slice(),
            b"---\n",
            b"---    ",
            b"---  \n",
            b"---\n  \n",
            b"---\n  \n",
        ] {
            let result = PatchDescription::try_from(description);
            assert!(result.is_err());
        }
    }
}