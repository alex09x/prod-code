/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[allow(dead_code)]
pub mod api {
    pub(in crate::api) trait Store: Send {}
    pub(self) trait Local: Send {}
    pub trait Crate: Send {}
    pub(super) trait Parent: Send {}
    pub(crate) trait Shared: Send {}

    #[allow(dead_code)]
    fn store_is_send<T: Store>(value: T) -> impl Send {
        value
    }
    #[allow(dead_code)]
    fn local_is_send<T: Local>(value: T) -> impl Send {
        value
    }
}

pub trait Shape<'a>: Send {
    fn shape(&self, value: &'a str) -> usize;
}
pub trait Higher
where
    for<'a> Self: Shape<'a>,
{
}
pub trait Mixed: Sync
where
    for<'a> Self: Shape<'a> + Send,
    Self: 'static,
{
}
pub trait Ordinary: Send
where
    Self: Sync,
{
}
pub trait Generic<T>
where
    for<'a> T: Shape<'a>,
{
}

pub struct Unit;
impl<'a> Shape<'a> for Unit {
    fn shape(&self, value: &'a str) -> usize {
        value.len()
    }
}
impl Higher for Unit {}
// This compiles only if Higher implies Shape for each independently chosen lifetime.
pub fn implied<'a, 'b, T: Higher>(value: &T, a: &'a str, b: &'b str) -> usize {
    value.shape(a) + value.shape(b)
}

pub trait Render: Send {}
#[allow(non_camel_case_types)]
pub trait r#impl {}
pub struct RawTarget;
impl r#impl for RawTarget {}

pub struct Target<A, B, C, D>(A, B, C, D);
#[rustfmt::skip]
impl<
    A: Send,
    B: Send,
    C: Send,
    D: Send,
> Render for Target<A, B, C, D> {}

pub struct Commented<T>(T);
#[rustfmt::skip]
impl<T: Send, // generic parameter
> Render
    for
    Commented<T>
{}

pub trait Tag<const C: char> {}
pub struct Tagged;
impl Tag<'x'> for Tagged {}
impl Tag<'y'> for Tagged {}
impl Tag<'>'> for Tagged {}
impl Tag<'{'> for Tagged {}
impl Tag<'🦀'> for Tagged {}
#[rustfmt::skip]
impl Tag /* for Fake { */ <'z'> for Tagged {}

pub struct Plain;
impl Plain {
    pub fn body() -> Plain {
        Plain
    }
}
pub struct After;
impl After {}
pub struct Unrelated;

pub struct Sentinel;
impl Render for Sentinel {}
pub struct Neighbor;
impl Sentinel {
    pub fn body() -> Sentinel {
        Sentinel
    }
}

/*
impl Fake for Unrelated {}
/* nested impl AlsoFake for Unrelated {} */
*/
pub const TEXT: &str = r##"
impl Invented for Unrelated {}
"##;
pub const QUOTED: &str = "impl Quoted for Unrelated {}";
pub fn use_unrelated() -> Unrelated {
    Unrelated
}

#[allow(dead_code, unused_variables)]
pub mod nested_const {
    trait Tag<const N: usize> {}
    pub struct Target;
    #[rustfmt::skip]
    impl Tag<{struct Inner; impl Inner {} 0}> for Target {}

    pub struct WithBody;
    #[rustfmt::skip]
    impl Tag<{struct Local; impl Local { fn body() { let value = 0; } } 1}> for WithBody {}
    pub struct Unrelated;
}

#[allow(dead_code)]
pub mod nested_derive {
    trait Tag<const N: usize> {}
    pub struct WithDerived;
    #[rustfmt::skip]
    impl Tag<{
        struct Inner;
        impl Inner {}
        #[derive(Clone)]
        struct DerivedLocal;
        0
    }> for WithDerived {}
}
