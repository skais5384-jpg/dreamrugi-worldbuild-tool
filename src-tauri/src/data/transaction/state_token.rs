//! 필드/원소 경계는 serde가 결정한다. 그 경계에서 값의 첫 token만 관찰한다.
use serde::de::{DeserializeOwned, DeserializeSeed, SeqAccess, Visitor};
use std::{
    cell::Cell,
    io::{self, Read},
    marker::PhantomData,
};

#[derive(Clone, Copy)]
pub(super) enum Shape {
    Number,
    String,
    Sequence,
    Enum,
    Unit,
}

#[derive(Default)]
pub(super) struct Tokens {
    expected: Cell<Option<Shape>>,
    last: Cell<Option<u8>>,
    pub(super) wrong_type: Cell<bool>,
}
impl Tokens {
    fn observe(&self, byte: u8) {
        if matches!(byte, b' ' | b'\n' | b'\r' | b'\t') {
            return;
        }
        let Some(shape) = self.expected.take() else {
            return;
        };
        // JSON 값이 시작되기 전 Syntax/EOF만으로 자료형을 추측하지 않는다.
        let token = matches!(
            byte,
            b'"' | b'[' | b'{' | b'n' | b't' | b'f' | b'-' | b'0'..=b'9'
        );
        let allowed = match shape {
            Shape::Number => matches!(byte, b'-' | b'0'..=b'9'),
            Shape::String => byte == b'"',
            Shape::Sequence => byte == b'[',
            // 기존 serde unit enum은 문자열과 외부 태그 객체를 받는다.
            Shape::Enum => matches!(byte, b'"' | b'{'),
            Shape::Unit => byte == b'n',
        };
        if token && !allowed {
            self.wrong_type.set(true);
        }
    }
}

pub(super) struct TokenReader<'a, R> {
    pub(super) buffered: R,
    pub(super) tokens: &'a Tokens,
}
impl<R: Read> Read for TokenReader<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        // 고정 BufReader *뒤*에서 serde에 건네는 한 byte만 관찰한다.
        // 파일 prefetch를 token 소비로 오인하거나 별도 handle/raw 사본을 만들지 않는다.
        let n = self.buffered.read(&mut out[..1])?;
        if n == 1 {
            self.tokens.last.set(Some(out[0]));
            self.tokens.observe(out[0]);
        }
        Ok(n)
    }
}

pub(super) struct Value<'a, T> {
    tokens: &'a Tokens,
    shape: Shape,
    peeked: bool,
    marker: PhantomData<T>,
}
impl<'a, T> Value<'a, T> {
    pub(super) fn field(tokens: &'a Tokens, shape: Shape) -> Self {
        Self {
            tokens,
            shape,
            peeked: false,
            marker: PhantomData,
        }
    }
}
impl<'de, T: DeserializeOwned> DeserializeSeed<'de> for Value<'_, T> {
    type Value = T;
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<T, D::Error> {
        self.tokens.expected.set(Some(self.shape));
        // 고정 serde_json 1.0.151: Map seed는 ':' 소비 직후, Seq seed는
        // has_next_element가 첫 token을 peek한 직후 호출한다. 내용 완성 전 관찰한다.
        if self.peeked {
            if let Some(byte) = self.tokens.last.get() {
                self.tokens.observe(byte);
            }
        }
        let result = T::deserialize(de);
        self.tokens.expected.set(None);
        result
    }
}

pub(super) struct Progress<'a>(pub(super) &'a Tokens);
impl<'de> DeserializeSeed<'de> for Progress<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<(), D::Error> {
        struct Continuous<'a>(&'a Tokens);
        impl<'de> Visitor<'de> for Continuous<'_> {
            type Value = ();
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("continuous progress")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
                let mut expected = 0u64;
                while let Some(index) = seq.next_element_seed(Value::<u32> {
                    tokens: self.0,
                    shape: Shape::Number,
                    peeked: true,
                    marker: PhantomData,
                })? {
                    if u64::from(index) != expected {
                        return Err(serde::de::Error::custom("invalid progress"));
                    }
                    expected += 1;
                }
                Ok(())
            }
        }
        self.0.expected.set(Some(Shape::Sequence));
        let result = de.deserialize_seq(Continuous(self.0));
        self.0.expected.set(None);
        result
    }
}

/// JSON의 외부 태그 경계만 감싼다. variant 이름과 해석은 모델의 derived serde가 소유한다.
pub(super) struct State<'a>(pub(super) &'a Tokens);
impl<'de> DeserializeSeed<'de> for State<'_> {
    type Value = super::TransactionState;
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        struct StateVisitor<'a>(&'a Tokens);
        impl<'de> Visitor<'de> for StateVisitor<'_> {
            type Value = super::TransactionState;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("transaction state enum")
            }
            fn visit_enum<A: serde::de::EnumAccess<'de>>(
                self,
                access: A,
            ) -> Result<Self::Value, A::Error> {
                // EnumAccessDeserializer를 통해 원래 모델 visitor에 그대로 넘긴다.
                // JSON은 name/variants 메타데이터를 사용하지 않으므로 목록을 중복 관리하지 않는다.
                serde::Deserialize::deserialize(serde::de::value::EnumAccessDeserializer::new(
                    StateAccess {
                        inner: access,
                        tokens: self.0,
                    },
                ))
            }
        }
        self.0.expected.set(Some(Shape::Enum));
        let result = de.deserialize_enum("TransactionState", &[], StateVisitor(self.0));
        self.0.expected.set(None);
        result
    }
}

struct StateAccess<'a, A> {
    inner: A,
    tokens: &'a Tokens,
}
impl<'de, 'a, A: serde::de::EnumAccess<'de>> serde::de::EnumAccess<'de> for StateAccess<'a, A> {
    type Error = A::Error;
    type Variant = StateAccess<'a, A::Variant>;
    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), A::Error> {
        let (value, inner) = self.inner.variant_seed(seed)?;
        Ok((
            value,
            StateAccess {
                inner,
                tokens: self.tokens,
            },
        ))
    }
}
impl<'de, A: serde::de::VariantAccess<'de>> serde::de::VariantAccess<'de> for StateAccess<'_, A> {
    type Error = A::Error;
    fn unit_variant(self) -> Result<(), A::Error> {
        // 고정 serde_json의 객체 variant_seed는 이름과 ':'까지만 소비했다.
        // unit 내부는 아직 peek하지 않았으므로 이전 last를 재사용하지 않는다.
        // 문자열형 variant는 여기서 읽지 않고 성공한다. 두 경우 모두 기대를 해제한다.
        self.tokens.expected.set(Some(Shape::Unit));
        let result = self.inner.unit_variant();
        self.tokens.expected.set(None);
        result
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, A::Error> {
        self.inner.newtype_variant_seed(seed)
    }
    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, A::Error> {
        self.inner.tuple_variant(len, visitor)
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, A::Error> {
        self.inner.struct_variant(fields, visitor)
    }
}
