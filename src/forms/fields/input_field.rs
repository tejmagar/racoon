use std::any::Any;
use std::marker::PhantomData;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::core::forms::{Files, FormData};

use crate::forms::fields::FieldResult;
use crate::forms::AbstractFields;

pub enum InputFieldError<'a> {
    MissingField(&'a String),
    /// (field_name, value, minimum_length)
    MinimumLengthRequired(&'a String, &'a String, &'a usize),
    /// (field_name, value, maximum_length)
    MaximumLengthExceed(&'a String, &'a String, &'a usize),
}

pub type ErrorHandler = Box<fn(InputFieldError, Vec<String>) -> Vec<String>>;

pub trait FromAny {
    fn from_vec(value: &mut Vec<String>) -> Option<Self>
    where
        Self: Sized;
}

impl FromAny for String {
    fn from_vec(values: &mut Vec<String>) -> Option<Self> {
        if values.len() > 0 {
            return Some(values.remove(0));
        }

        // Must return String.
        // Here None denotes values cannot be correctly converted to type T.
        None
    }
}

impl FromAny for Option<String> {
    fn from_vec(values: &mut Vec<String>) -> Option<Self> {
        if values.len() > 0 {
            let value = values.remove(0);
            return Some(Some(value));
        } else {
            // Here outer Some denotes values are correctly converted to type T with value None.
            // Since fields are missing, default value is None.
            return Some(None);
        }
    }
}

pub struct InputField<T> {
    field_name: String,
    max_length: Option<Arc<usize>>,
    result: Arc<Mutex<Option<Box<dyn Any + Send + Sync + 'static>>>>,
    error_handler: Option<Arc<ErrorHandler>>,
    default_value: Option<String>,
    validated: Arc<AtomicBool>,
    phantom: PhantomData<T>,
}

impl<T: FromAny + Sync + Send + 'static> InputField<T> {
    pub fn new<S: AsRef<str>>(field_name: S) -> Self {
        let field_name = field_name.as_ref().to_string();

        Self {
            field_name,
            max_length: None,
            result: Arc::new(Mutex::new(Some(Box::new(None::<String>)))),
            error_handler: None,
            default_value: None,
            validated: Arc::new(AtomicBool::from(false)),
            phantom: PhantomData,
        }
    }

    pub fn max_length(mut self, max_length: usize) -> Self {
        self.max_length = Some(Arc::new(max_length));
        self
    }

    pub fn set_default<S: AsRef<str>>(mut self, value: S) -> Self {
        // Makes field optional
        let value = value.as_ref().to_string();
        self.default_value = Some(value);
        self
    }

    pub fn handle_error_message(
        mut self,
        callback: fn(InputFieldError, Vec<String>) -> Vec<String>,
    ) -> Self {
        let callback = Arc::new(Box::new(callback));
        self.error_handler = Some(callback);
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
                    let t = *t;
                    return t;
                }

                _ => {}
            };
        }

        panic!("Unexpected error. Bug in input_field.rs file.");
    }
}
fn validate_input_length(
    field_name: &String,
    values: &Vec<String>,
    error_handler: Option<Arc<ErrorHandler>>,
    max_length: Option<Arc<usize>>,
    errors: &mut Vec<String>,
) {
    let value;
    if let Some(value_ref) = values.get(0) {
        value = value_ref;
    } else {
        return;
    }

    if let Some(max_length) = max_length {
        // Checks maximum value length constraints
        if value.len() > *max_length {
            let default_max_length_exceed_messsage =
                format!("Character length exceeds maximum size of {}", *max_length);

            if let Some(error_handler) = error_handler {
                let max_length_exceed_error =
                    InputFieldError::MaximumLengthExceed(&value, &field_name, &max_length);

                let custom_errors = error_handler(
                    max_length_exceed_error,
                    vec![default_max_length_exceed_messsage],
                );
                errors.extend(custom_errors);
            } else {
                errors.push(default_max_length_exceed_messsage);
            }
        }
    }
}

impl<T: FromAny> Clone for InputField<T> {
    fn clone(&self) -> Self {
        Self {
            field_name: self.field_name.clone(),
            max_length: self.max_length.clone(),
            error_handler: self.error_handler.clone(),
            result: self.result.clone(),
            default_value: self.default_value.clone(),
            validated: self.validated.clone(),
            phantom: self.phantom.clone(),
        }
    }
}

impl<T: FromAny + Sync + Send + 'static> AbstractFields for InputField<T> {
    fn field_name(&self) -> FieldResult<String> {
        let field_name = self.field_name.clone();
        Box::new(Box::pin(async move { field_name }))
    }

    fn validate(
        &mut self,
        form_data: &mut FormData,
        _: &mut Files,
    ) -> FieldResult<Result<(), Vec<String>>> {
        let field_name = self.field_name.clone();

        let mut form_values;

        // Takes value from form field
        if let Some(values) = form_data.remove(&field_name) {
            form_values = Some(values);
        } else {
            form_values = None;
        }

        let max_length = self.max_length.clone();
        let default_value = self.default_value.take();
        let validated = self.validated.clone();
        let result = self.result.clone();

        let error_handler = self.error_handler.clone();

        Box::new(Box::pin(async move {
            let mut errors: Vec<String> = vec![];

            let is_empty;
            if let Some(values) = form_values.as_mut() {
                validate_input_length(
                    &field_name,
                    &values,
                    error_handler.clone(),
                    max_length,
                    &mut errors,
                );

                is_empty = values.is_empty();
            } else {
                is_empty = true;
            }

            // Handles field missing error.
            let is_optional =
                std::any::TypeId::of::<T>() == std::any::TypeId::of::<Option<String>>();

            if !is_optional && is_empty {
                // If default value is specified, set default value for value
                if let Some(default_value) = default_value {
                    if is_empty {
                        form_values = Some(vec![default_value]);
                    }
                } else {
                    let default_field_missing_error = "This field is missing.".to_string();

                    if let Some(error_handler) = error_handler {
                        let field_missing_error = InputFieldError::MissingField(&field_name);
                        let custom_errors =
                            error_handler(field_missing_error, vec![default_field_missing_error]);
                        errors.extend(custom_errors);
                    } else {
                        errors.push(default_field_missing_error);
                    }
                }
            }

            if errors.len() > 0 {
                return Err(errors);
            }

            // All the validation conditions are satisfied.
            validated.store(true, Ordering::Relaxed);
            {
                let mut result_lock = result.lock().await;
                if let Some(values) = form_values.as_mut() {
                    let value_t = T::from_vec(values);
                    *result_lock = Some(Box::new(value_t.unwrap()));
                } else {
                    // Above conditions are satisfied however there are no values stored.
                    // Probably Optional type without default value.
                    let value_t = T::from_vec(&mut vec![]);
                    *result_lock = Some(Box::new(value_t.unwrap()));
                }
            }
            Ok(())
        }))
    }

    fn wrap(&self) -> Box<dyn AbstractFields> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
pub mod test {
    use crate::core::forms::{Files, FormData};
    use crate::forms::fields::AbstractFields;

    use super::InputField;

    #[tokio::test]
    async fn test_validate_default() {
        let mut form_data = FormData::new();
        let mut files = Files::new();

        let mut input_field: InputField<String> =
            InputField::new("name").set_default("John").max_length(100);
        let result = input_field.validate(&mut form_data, &mut files).await;
        assert_eq!(true, result.is_ok());

        let value = input_field.value().await;
        assert_eq!(value, "John");
    }

    #[tokio::test]
    async fn test_validate_string() {
        let mut form_data = FormData::new();
        form_data.insert("name".to_string(), vec!["John".to_string()]);

        let mut files = Files::new();

        let mut input_field: InputField<String> = InputField::new("name").max_length(100);
        let result = input_field.validate(&mut form_data, &mut files).await;
        assert_eq!(true, result.is_ok());

        let value = input_field.value().await;
        assert_eq!(value, "John");
    }

    #[tokio::test]
    async fn test_validate_optional() {
        let mut form_data = FormData::new();
        let mut files = Files::new();

        let mut input_field: InputField<Option<String>> = InputField::new("name").max_length(100);
        let result = input_field.validate(&mut form_data, &mut files).await;
        assert_eq!(true, result.is_ok());

        let value = input_field.value().await;
        assert_eq!(value, None);

        // With values
        form_data.insert("name".to_string(), vec!["John".to_string()]);
        let mut input_field2: InputField<Option<String>> = InputField::new("name").max_length(100);
        let result = input_field2.validate(&mut form_data, &mut files).await;
        assert_eq!(true, result.is_ok());
        assert_eq!(Some("John".to_string()), input_field2.value().await);
    }

    #[tokio::test]
    async fn test_validation_error() {
        let mut input_field: InputField<String> = InputField::new("name").max_length(100);
        let mut form_data = FormData::new();
        let mut files = Files::new();
        let result = input_field.validate(&mut form_data, &mut files).await;
        assert_eq!(false, result.is_ok());
    }
}
