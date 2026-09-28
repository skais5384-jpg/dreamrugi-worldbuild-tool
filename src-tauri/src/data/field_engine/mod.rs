//! Field 값의 순수 의미 계층이다.
//!
//! 이 계층은 artifact wire 형식이나 저장소를 알지 못한다. 이후 Field Engine은 여기서
//! 검증된 값을 재사용하고, artifact codec은 영속 경계에서 같은 규칙을 적용한다.

pub(crate) mod choice;
pub(crate) mod rich_text;
pub(crate) mod scalar;
pub(crate) mod validation;
