mod engine;
mod errors;

use pyo3::prelude::*;

macro_rules! register_exceptions {
    ($module:expr, [$($name:literal => $ty:ty),+ $(,)?]) => {
        $($module.add($name, $module.py().get_type::<$ty>())?;)+
    };
}

#[pymodule]
fn lattice(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", "0.1.0")?;

    register_exceptions!(
        m,
        [
            "LatticeError" => errors::LatticeError,
            "RateLimitError" => errors::RateLimitError,
            "AuthenticationError" => errors::AuthenticationError,
            "ModelNotFoundError" => errors::ModelNotFoundError,
            "ProviderUnavailableError" => errors::ProviderUnavailableError,
            "ContextWindowExceededError" => errors::ContextWindowExceededError,
            "ToolExecutionError" => errors::ToolExecutionError,
            "StreamingError" => errors::StreamingError,
            "ConfigError" => errors::ConfigError,
            "NetworkError" => errors::NetworkError,
        ]
    );

    m.add_class::<engine::LatticeEngine>()?;
    m.add_class::<engine::PyResolvedModel>()?;
    m.add_class::<engine::StreamIterator>()?;

    Ok(())
}
