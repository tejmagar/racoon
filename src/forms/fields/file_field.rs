use std::any::Any;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_tempfile::TempFile;
use tokio::sync::Mutex;

use crate::core::forms::{Files, FormData};
use crate::forms::AbstractFields;

use crate::forms::fields::FieldResult;

pub struct UploadedFile {
    pub filename: String,
    core_file_field: crate::core::forms::FileField,
    pub temp_path: PathBuf,
}

impl UploadedFile {
    pub fn from_core_file_field(core_file_field: crate::core::forms::FileField) -> Self {
        let temp_path = core_file_field.temp_path.clone();
        let filename = core_file_field.name.clone();

        Self {
            filename,
            core_file_field,
            temp_path,
        }
    }

    pub fn core_file_field(&mut self) -> &crate::core::forms::FileField {
        &mut self.core_file_field
    }

    pub fn from_temp_file<S: AsRef<str>>(filename: S, temp_file: TempFile) -> Self {
        let filename = filename.as_ref().to_string();
        let core_file_field = crate::core::forms::FileField::from(&filename, temp_file);
        let temp_path = core_file_field.temp_path.clone();

        Self {
            filename,
            core_file_field,
            temp_path,
        }
    }
}

pub type PostValidator<T> = Box<fn(T) -> Result<T, Vec<String>>>;
type BoxResult = Box<dyn Any + Sync + Send + 'static>;

pub struct FileField<T> {
    field_name: String,
    result: Arc<Mutex<Option<BoxResult>>>,
    post_validator: Option<PostValidator<T>>,
    validated: Arc<AtomicBool>,
    phantom: PhantomData<T>,
}

impl<T> Clone for FileField<T> {
    fn clone(&self) -> Self {
        Self {
            field_name: self.field_name.clone(),
            result: self.result.clone(),
            post_validator: self.post_validator.clone(),
            validated: self.validated.clone(),
            phantom: self.phantom.clone(),
        }
    }
}

pub trait ToOptionT {
    fn from_vec(files: &mut Vec<crate::core::forms::FileField>) -> Option<Self>
    where
        Self: Sized;

    fn is_optional() -> bool;
}

impl ToOptionT for UploadedFile {
    fn from_vec(files: &mut Vec<crate::core::forms::FileField>) -> Option<Self> {
        if files.len() > 0 {
            let file_field = files.remove(0);
            return Some(UploadedFile::from_core_file_field(file_field));
        }

        None
    }

    fn is_optional() -> bool {
        false
    }
}

impl ToOptionT for Option<UploadedFile> {
    fn from_vec(files: &mut Vec<crate::core::forms::FileField>) -> Option<Self> {
        if files.len() > 0 {
            let file_field = files.remove(0);
            // Outer Some denotes successful conversion.
            return Some(Some(UploadedFile::from_core_file_field(file_field)));
        }

        // Return successful conversion but no files are present. So returns actual value as None.
        Some(None)
    }

    fn is_optional() -> bool {
        true
    }
}

impl ToOptionT for Vec<UploadedFile> {
    fn from_vec(files: &mut Vec<crate::core::forms::FileField>) -> Option<Self>
    where
        Self: Sized,
    {
        if files.len() > 0 {
            let mut owned_files = vec![];

            for i in (0..files.len()).rev() {
                let uploaded_file = UploadedFile::from_core_file_field(files.remove(i));
                owned_files.insert(0, uploaded_file);
            }

            return Some(owned_files);
        }

        // Conversion to type T failed.
        None
    }

    fn is_optional() -> bool {
        false
    }
}

impl ToOptionT for Option<Vec<UploadedFile>> {
    fn from_vec(files: &mut Vec<crate::core::forms::FileField>) -> Option<Self>
    where
        Self: Sized,
    {
        if files.len() > 0 {
            let mut owned_files = vec![];

            for i in (0..files.len()).rev() {
                let uploaded_file = UploadedFile::from_core_file_field(files.remove(i));
                owned_files.insert(0, uploaded_file);
            }

            return Some(Some(owned_files));
        }

        // Conversion to type T successful because of optional field. So returns None as result.
        Some(None)
    }

    fn is_optional() -> bool {
        true
    }
}

impl<T: Sync + Send + 'static> FileField<T> {
    pub fn new<S: AsRef<str>>(field_name: S) -> Self {
        let field_name = field_name.as_ref().to_string();
        Self {
            field_name,
            result: Arc::new(Mutex::new(None)),
            post_validator: None,
            validated: Arc::new(AtomicBool::from(false)),
            phantom: PhantomData,
        }
    }

    pub fn post_validate(mut self, callback: fn(T) -> Result<T, Vec<String>>) -> Self {
        self.post_validator = Some(Box::new(callback));
        self
    }

    pub async fn value(self) -> T {
        if !self.validated.load(Ordering::Relaxed) {
            panic!("This field is not validated. Please call form.validate() method before accessing value.");
        }

        let mut result_ref = self.result.lock().await;
        let result = result_ref.take();

        if let Some(result) = result {
            match result.downcast::<T>() {
                Ok(t) => {
                    return *t;
                }

                _ => {}
            };
        }

        panic!("Unexpected error. Bug in file_field.rs file.");
    }
}

impl<T: ToOptionT + Sync + Send + 'static> AbstractFields for FileField<T> {
    fn field_name(&self) -> FieldResult<String> {
        let field_name = self.field_name.clone();
        Box::new(Box::pin(async move { field_name }))
    }

    fn validate(
        &mut self,
        _: &mut FormData,
        files: &mut Files,
    ) -> FieldResult<Result<(), Vec<String>>> {
        let files = files.remove(&self.field_name);
        let result_ref = self.result.clone();
        let validated = self.validated.clone();
        let post_validator = self.post_validator.clone();

        Box::new(Box::pin(async move {
            let mut errors = vec![];

            let is_optional = T::is_optional();

            let is_empty;

            if let Some(mut files) = files {
                let mut result = result_ref.lock().await;
                is_empty = files.is_empty();

                if let Some(t) = T::from_vec(&mut files) {
                    if let Some(post_validator) = post_validator {
                        match post_validator(t) {
                            Ok(t) => {
                                *result = Some(Box::new(t));
                            }
                            Err(custom_errors) => {
                                errors.extend_from_slice(&custom_errors);
                            }
                        }
                    } else {
                        *result = Some(Box::new(t));
                    }
                }
            } else {
                is_empty = true;
            }

            if !is_optional && is_empty {
                errors.push("This field is required.".to_string());
            }

            if errors.len() > 0 {
                return Err(errors);
            }

            if is_optional && is_empty {
                let value_t = T::from_vec(&mut vec![]);
                if let Some(t) = value_t {
                    let mut result = result_ref.lock().await;
                    *result = Some(Box::new(t));
                }
            }

            validated.store(true, Ordering::Relaxed);
            Ok(())
        }))
    }

    fn wrap(&self) -> Box<dyn AbstractFields> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
pub mod tests {
    use async_tempfile::TempFile;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::core::forms::{Files, FormData};
    use crate::forms::fields::AbstractFields;

    use super::{FileField, UploadedFile};

    #[tokio::test]
    async fn test_file_optional() {
        let mut form_data = FormData::new();
        let mut files = Files::new();

        let mut file_field: FileField<Option<UploadedFile>> = FileField::new("file");
        let result = file_field.validate(&mut form_data, &mut files).await;

        assert_eq!(true, result.is_ok());
    }

    #[tokio::test]
    async fn test_file_empty() {
        let mut form_data = FormData::new();
        let mut files = Files::new();

        let mut file_field: FileField<UploadedFile> = FileField::new("file");
        let result = file_field.validate(&mut form_data, &mut files).await;

        assert_eq!(false, result.is_ok());
    }

    #[tokio::test]
    async fn test_file_validate() {
        let mut form_data = FormData::new();
        let mut files = Files::new();

        let mut temp_file = TempFile::new().await.unwrap();
        let _ = temp_file.write_all(b"Hello World").await;

        let core_file_field = crate::core::forms::FileField::from(
            "file.txt".to_string(),
            temp_file,
        );

        let mut file_field: FileField<UploadedFile> = FileField::new("file");
        files.insert("file".to_string(), vec![core_file_field]);
        let result = file_field.validate(&mut form_data, &mut files).await;

        let path_field = file_field.value().await;
        let path_buf = path_field.temp_path;

        assert_eq!(true, path_buf.exists());
        assert_eq!(true, result.is_ok());

        let mut file = tokio::fs::File::open(&path_buf).await.unwrap();
        let mut content = String::new();
        let _ = file.read_to_string(&mut content).await;
        assert_eq!("Hello World".to_string(), content);
    }

    #[tokio::test]
    async fn test_file_validate_vec() {
        let mut form_data = FormData::new();
        let mut files = Files::new();

        let temp_file = TempFile::new().await.unwrap();
        let core_file_field = crate::core::forms::FileField::from("file.txt", temp_file);

        let mut file_field: FileField<Vec<UploadedFile>> = FileField::new("file");
        files.insert("file".to_string(), vec![core_file_field]);
        let result = file_field.validate(&mut form_data, &mut files).await;
        assert_eq!(true, result.is_ok());

        let sent_files = file_field.value().await;
        assert_eq!(1, sent_files.len());
    }

    #[tokio::test]
    async fn test_file_validate_vec_optional() {
        let mut form_data = FormData::new();
        let mut files = Files::new();

        let temp_file = TempFile::new().await.unwrap();
        let core_file_field = crate::core::forms::FileField::from(
            "file.txt".to_string(),
            temp_file,
        );

        let mut file_field: FileField<Option<Vec<UploadedFile>>> = FileField::new("file");
        files.insert("file".to_string(), vec![core_file_field]);
        let result = file_field.validate(&mut form_data, &mut files).await;
        assert_eq!(true, result.is_ok());

        let sent_files = file_field.value().await;
        assert_eq!(true, sent_files.is_some());
        assert_eq!(1, sent_files.unwrap().len());

        // Empty test

        let mut form_data = FormData::new();
        let mut files = Files::new();

        let mut file_field: FileField<Option<Vec<UploadedFile>>> = FileField::new("file");
        let result = file_field.validate(&mut form_data, &mut files).await;
        assert_eq!(true, result.is_ok());
        assert_eq!(false, file_field.value().await.is_some());
    }

    #[tokio::test]
    async fn test_post_validate() {
        let mut form_data = FormData::new();
        let mut files = Files::new();

        let temp_file = TempFile::new().await.unwrap();
        let core_file_field = crate::core::forms::FileField::from(
            "file.txt".to_string(),
            temp_file,
        );

        let mut file_field: FileField<UploadedFile> =
            FileField::new("file").post_validate(|file| {
                if !file.filename.eq("file2.txt") {
                    return Err(vec!["File name does not equal file2.txt".to_string()]);
                }

                Ok(file)
            });
        files.insert("file".to_string(), vec![core_file_field]);
        let result = file_field.validate(&mut form_data, &mut files).await;
        assert_eq!(false, result.is_ok());
    }
}
