use proc_macro::{TokenStream, TokenTree};
use quote::{ToTokens, quote};
use syn::{Arm, Fields, GenericArgument, ItemEnum, PathArguments, Type, Variant};

/// Generates registers from the register CLI enum
///
/// This needs to be done as the register enums aren't fully compatible with each other, so this
/// allows you to automatically generate the register enum actually used by communication crate
///
/// Required arguments are `unsupported`, `to` and `newtype`, all being identifiers
#[proc_macro_attribute]
pub fn generate_registers(attr: TokenStream, item: TokenStream) -> TokenStream {
    enum AttrArgs {
        Unsupported(Type),
        NewType(Type),
        To(Type),
    }

    // Parse all the incoming arguments
    let arguments: Vec<TokenTree> = attr.into_iter().collect();
    let arguments: Vec<AttrArgs> = arguments
        .into_iter()
        .filter(|tree| !matches!(tree, TokenTree::Punct(punct) if punct.to_string() == ","))
        .collect::<Vec<TokenTree>>()
        .chunks(3)
        .map(|chunk| match chunk {
            [
                TokenTree::Ident(name),
                TokenTree::Punct(punct),
                TokenTree::Ident(ty),
            ] if punct.to_string() == "=" => (name.to_owned(), ty.to_owned()),
            _ => panic!("Unsupported syntax"),
        })
        .map(|(ident, ty)| {
            let stream = TokenStream::from(TokenTree::Ident(ty));
            let ty: Type = syn::parse(stream).expect("Expected type after =");
            match ident.to_string().as_str() {
                "unsupported" => AttrArgs::Unsupported(ty),
                "to" => AttrArgs::To(ty),
                "newtype" => AttrArgs::NewType(ty),
                _ => panic!("Invalid key"),
            }
        })
        .collect();

    // Check if all the expected arguments are there
    let new_type = arguments
        .iter()
        .find_map(|arg| match arg {
            AttrArgs::NewType(ty) => Some(ty),
            _ => None,
        })
        .expect("Need newtype to generate");
    let to_container = arguments
        .iter()
        .find_map(|arg| match arg {
            AttrArgs::To(ty) => Some(ty),
            _ => None,
        })
        .expect("Need type to convert to");
    let to_container_name = to_container.to_token_stream().to_string();
    let unsupported_container = arguments
        .iter()
        .find_map(|arg| match arg {
            AttrArgs::Unsupported(ty) => Some(ty),
            _ => None,
        })
        .expect("Need type marked as unsupported");

    let cli_enum: ItemEnum = syn::parse(item).expect("whoopsie daisy");
    let cli_enum_ident = cli_enum.ident.clone();
    let cli_enum_name = cli_enum.ident.to_string();

    // Generate the original enum variant, the new enum variant and the match cases to convert from A -> B
    let (cli_variants, new_variants, match_cases): (Vec<Variant>, Vec<Variant>, Vec<Arm>) = cli_enum.variants.into_iter().map(|variant| {
        let variant_name = variant.ident.clone();

        match variant.fields.clone() {
            Fields::Unnamed(unnamed) => {
                let field = unnamed.unnamed.get(0).expect("Expects one field");
                let Type::Path(type_path) = field.ty.clone() else {
                    panic!(
                        "Invalid type given, expected TypePath (not to be confused with the Path type)"
                    )
                };

                let PathArguments::AngleBracketed(arguments) = type_path
                    .path
                    .segments
                    .last()
                    .expect("Invalid type, missing path segments")
                    .arguments
                    .clone()
                else {
                    panic!(
                        "Invalid type, must be a type with angle bracket arguments (aka ReadWrite<L, R>"
                    )
                };
                let arguments: Vec<Type> = arguments
                    .args
                    .into_iter()
                    .map(|arg| {
                        let GenericArgument::Type(ty) = arg else {
                            panic!("Only supports types within angle brackets")
                        };
                        ty
                    })
                    .collect();

                #[derive(Debug, Clone, Hash, PartialEq, Eq)]
                enum RegisterType {
                    None,
                    Read(Type),
                    Write(Type),
                    Both(Type, Type),
                }

                let reg_type = match arguments.as_slice() {
                    [l, r] if l == unsupported_container && r == unsupported_container => {
                        RegisterType::None
                    }
                    [l, r] if l == unsupported_container && r != unsupported_container => {
                        RegisterType::Write(r.clone())
                    }
                    [l, r] if l != unsupported_container && r == unsupported_container => {
                        RegisterType::Read(l.clone())
                    }
                    [l, r] if l != unsupported_container && r != unsupported_container => {
                        RegisterType::Both(l.clone(), r.clone())
                    }
                    _ => panic!("Type MUST have to arguments in angle brackets"),
                };


                let cli_item = match reg_type.clone() {
                    RegisterType::None => {
                        let variant_attributes = variant.attrs.iter().map(|attr| attr.to_token_stream().into()).fold(TokenStream::new(), |mut acc, attr: TokenStream| {
                            acc.extend(attr);
                            acc
                        });
                        format!("{variant_attributes}\n{variant_name}")
                    },
                    _ => variant.to_token_stream().to_string(),
                };

                let reg_item = match reg_type.clone() {
                    RegisterType::None => variant_name.to_string(),
                    RegisterType::Read(ty) => format!(
                        "{variant_name}(<{} as ArgumentContainer>::Contains)",
                        ty.to_token_stream()
                    ),
                    RegisterType::Write(ty) => format!(
                        "{variant_name}(<{} as ArgumentContainer>::Contains)",
                        ty.to_token_stream()
                    ),
                    RegisterType::Both(r, w) => format!(
                        "{variant_name}({to_container_name}<<{} as ArgumentContainer>::Contains, <{} as ArgumentContainer>::Contains>)",
                        r.to_token_stream(),
                        w.to_token_stream()
                    ),
                };

                let match_case = match reg_type.clone() {
                    RegisterType::None => format!("{cli_enum_name}::{variant_name} => Self::{variant_name}"),
                    RegisterType::Read(_) => format!("{cli_enum_name}::{variant_name}(arg) => Self::{variant_name}(arg.coerce_read())"),
                    RegisterType::Write(_) => format!("{cli_enum_name}::{variant_name}(arg) => Self::{variant_name}(arg.coerce_write())"),
                    RegisterType::Both(_, _) => format!("{cli_enum_name}::{variant_name}(arg) => Self::{variant_name}(arg.into())"),
                };

                let cli_item: Variant = syn::parse_str(&cli_item)
                    .expect("Failed to generate original variant");
                let reg_item: Variant = syn::parse_str(&reg_item)
                    .expect("Failed to generate new variant");
                let match_case: Arm = syn::parse_str(&match_case)
                    .expect("Failed to generate match case");

                (cli_item, reg_item, match_case)
            }
            _ => panic!("Only supports unnamed fields enums"),
        }
    }).fold((Vec::new(), Vec::new(), Vec::new()), |mut acc, item| {
        acc.0.push(item.0);
        acc.1.push(item.1);
        acc.2.push(item.2);
        acc
    });

    quote! {
        /// All the possible registers the CLI may write to
        #[derive(Subcommand, Debug)]
        pub enum #cli_enum_ident {
            #(#cli_variants),*
        }

        /// An enum for operating on different registers on the ILA, without having to know the explicit
        /// address.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum #new_type {
            #(#new_variants),*
        }

        impl From<#cli_enum_ident> for #new_type {
            fn from(value: #cli_enum_ident) -> Self {
                match value {
                    #(#match_cases),*
                }
            }
        }
    }.into()
}
