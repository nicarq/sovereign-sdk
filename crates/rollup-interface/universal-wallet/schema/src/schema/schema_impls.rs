#[doc(hidden)]
// This module contains the implementations of the `SchemaGenerator` trait for all primitive types
mod primitive_type_impls {
    extern crate alloc;
    use std::any::TypeId;
    use std::cell::{Cell, RefCell, UnsafeCell};
    use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
    use std::marker::PhantomData;
    use std::ops::Range;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex, RwLock};

    use crate::schema::container::Container;
    use crate::schema::{
        IndexLinking, Item, Link, OverrideSchema, Primitive, Schema, SchemaGenerator,
    };
    use crate::ty::{
        ByteDisplay, EnumVariant, IntegerDisplay, IntegerType, NamedField, Struct, Tuple,
        UnnamedField,
    };

    macro_rules! impl_is_primitive_for_int {
        ($t:ident) => {
            impl SchemaGenerator for $t {
                fn scaffold() -> Item<IndexLinking> {
                    Item::Atom(Primitive::Integer(IntegerType::$t, IntegerDisplay::Decimal))
                }
                fn get_child_links<M>(_schema: &mut Schema<M>) -> Vec<Link> {
                    Vec::new()
                }
            }
        };
    }
    impl_is_primitive_for_int!(u8);
    impl_is_primitive_for_int!(u16);
    impl_is_primitive_for_int!(u32);
    impl_is_primitive_for_int!(u64);
    impl_is_primitive_for_int!(u128);
    impl_is_primitive_for_int!(i8);
    impl_is_primitive_for_int!(i16);
    impl_is_primitive_for_int!(i32);
    impl_is_primitive_for_int!(i64);
    impl_is_primitive_for_int!(i128);

    impl OverrideSchema for usize {
        type Output = u32;
    }

    impl OverrideSchema for isize {
        type Output = u32;
    }

    impl SchemaGenerator for bool {
        fn scaffold() -> Item<IndexLinking> {
            Item::Atom(Primitive::Boolean)
        }
        fn get_child_links<M>(_schema: &mut Schema<M>) -> Vec<Link> {
            Vec::new()
        }
    }

    impl SchemaGenerator for f32 {
        fn scaffold() -> Item<IndexLinking> {
            Item::Atom(Primitive::Float32)
        }
        fn get_child_links<M>(_schema: &mut Schema<M>) -> Vec<Link> {
            Vec::new()
        }
    }

    impl SchemaGenerator for f64 {
        fn scaffold() -> Item<IndexLinking> {
            Item::Atom(Primitive::Float64)
        }
        fn get_child_links<M>(_schema: &mut Schema<M>) -> Vec<Link> {
            Vec::new()
        }
    }

    impl<T: 'static> SchemaGenerator for PhantomData<T> {
        fn scaffold() -> Item<IndexLinking> {
            Item::Atom(Primitive::Skip { len: 0 })
        }
        fn get_child_links<M>(_schema: &mut Schema<M>) -> Vec<Link> {
            Vec::new()
        }
    }

    impl SchemaGenerator for String {
        fn scaffold() -> Item<IndexLinking> {
            Item::Atom(Primitive::String)
        }
        fn get_child_links<M>(_schema: &mut Schema<M>) -> Vec<Link> {
            Vec::new()
        }
    }

    impl<'a> OverrideSchema for &'a str {
        type Output = String;
    }

    impl<T: SchemaGenerator> SchemaGenerator for Vec<T> {
        fn get_child_links<M>(schema: &mut Schema<M>) -> Vec<Link> {
            vec![T::make_linkable(schema)]
        }

        fn scaffold() -> Item<IndexLinking> {
            if TypeId::of::<T>() == TypeId::of::<u8>() {
                Item::Atom(Primitive::ByteVec {
                    display: ByteDisplay::Hex,
                })
            } else {
                Item::Container(Container::Vec {
                    value: Link::Placeholder,
                })
            }
        }
    }

    impl<T: SchemaGenerator> OverrideSchema for HashSet<T> {
        type Output = Vec<T>;
    }

    impl<T: SchemaGenerator> OverrideSchema for BTreeSet<T> {
        type Output = Vec<T>;
    }

    impl<const N: usize, T: SchemaGenerator> SchemaGenerator for [T; N] {
        fn get_child_links<M>(schema: &mut Schema<M>) -> Vec<Link> {
            vec![T::make_linkable(schema)]
        }

        fn scaffold() -> Item<IndexLinking> {
            if TypeId::of::<T>() == TypeId::of::<u8>() {
                Item::Atom(Primitive::ByteArray {
                    len: N,
                    display: ByteDisplay::Hex,
                })
            } else {
                Item::Container(Container::Array {
                    len: N,
                    value: Link::Placeholder,
                })
            }
        }
    }

    macro_rules! impl_container_type {
        ($t:ident) => {
            impl<T: SchemaGenerator> OverrideSchema for $t<T> {
                type Output = T;
            }
        };
    }
    impl_container_type!(Box);
    impl_container_type!(Arc);
    impl_container_type!(Rc);
    impl_container_type!(Cell);
    impl_container_type!(RefCell);
    impl_container_type!(UnsafeCell);
    impl_container_type!(Mutex);
    impl_container_type!(RwLock);

    /// Helper macro, used for counting repetition without actually using the type
    macro_rules! type_to_placeholder {
        ($t: tt) => {
            Link::Placeholder
        };
    }
    macro_rules! impl_tuple_type {
        ($($tts:tt),*) => {
            impl<$($tts: SchemaGenerator + 'static,)*> SchemaGenerator for ($($tts,)*) {
                fn scaffold() -> Item<IndexLinking> {
                    Item::Container(Container::Tuple(Tuple {
                        fields: vec![
                            $(UnnamedField {
                                value: type_to_placeholder!($tts),
                                silent: false,
                                doc: "".to_string()
                            }),*
                        ]
                    }))
                }

                fn get_child_links<M>(schema: &mut Schema<M>) -> Vec<Link> {
                    vec![$($tts::make_linkable(schema)),*]
                }
            }
        };
    }
    // TODO: generate this with a proc macro?
    impl_tuple_type!(T1, T2);
    impl_tuple_type!(T1, T2, T3);
    impl_tuple_type!(T1, T2, T3, T4);
    impl_tuple_type!(T1, T2, T3, T4, T5);
    impl_tuple_type!(T1, T2, T3, T4, T5, T6);
    impl_tuple_type!(T1, T2, T3, T4, T5, T6, T7);
    impl_tuple_type!(T1, T2, T3, T4, T5, T6, T7, T8);

    impl<T: SchemaGenerator> SchemaGenerator for Option<T> {
        fn scaffold() -> Item<IndexLinking> {
            Item::Container(Container::Enum(crate::ty::Enum {
                type_name: "Option".to_string(),
                serde_type_name: "Option".to_string(),
                variants: vec![
                    EnumVariant {
                        name: "Some".to_string(),
                        serde_name: "Some".to_string(),
                        value: Some(Link::Placeholder),
                    },
                    EnumVariant {
                        name: "None".to_string(),
                        serde_name: "None".to_string(),
                        value: None,
                    },
                ],
            }))
        }

        fn get_child_links<M>(schema: &mut Schema<M>) -> Vec<Link> {
            vec![T::make_linkable(schema)]
        }
    }

    impl<K: SchemaGenerator, V: SchemaGenerator> SchemaGenerator for HashMap<K, V> {
        fn scaffold() -> Item<IndexLinking> {
            Item::Container(Container::Map {
                key: Link::Placeholder,
                value: Link::Placeholder,
            })
        }

        fn get_child_links<M>(schema: &mut Schema<M>) -> Vec<Link> {
            vec![K::make_linkable(schema), V::make_linkable(schema)]
        }
    }

    impl<K: SchemaGenerator, V: SchemaGenerator> OverrideSchema for BTreeMap<K, V> {
        type Output = HashMap<K, V>;
    }

    impl<T: SchemaGenerator> SchemaGenerator for Range<T> {
        fn scaffold() -> Item<IndexLinking> {
            Item::Container(Container::Struct(Struct {
                type_name: "Range".to_string(),
                serde_type_name: "Range".to_string(),
                fields: vec![
                    NamedField {
                        value: Link::Placeholder,
                        doc: "".to_string(),
                        silent: false,
                        display_name: "start".to_string(),
                        serde_display_name: "start".to_string(),
                    },
                    NamedField {
                        value: Link::Placeholder,
                        doc: "".to_string(),
                        silent: false,
                        display_name: "end".to_string(),
                        serde_display_name: "end".to_string(),
                    },
                ],
            }))
        }

        fn get_child_links<M>(schema: &mut Schema<M>) -> Vec<Link> {
            vec![T::make_linkable(schema), T::make_linkable(schema)]
        }
    }
}
