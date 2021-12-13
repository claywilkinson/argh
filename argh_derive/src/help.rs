// Copyright (c) 2020 Google LLC All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

use {
    crate::{
        errors::Errors,
        parse_attrs::{Description, FieldKind, TypeAttrs},
        Optionality, StructField,
    },
    argh_shared::INDENT,
    proc_macro2::{Span, TokenStream},
    quote::quote,
};

const SECTION_SEPARATOR: &str = "\n\n";

// Define constants for strings used for both help formats.
const HELP_FLAG: &str = "--help";
const HELP_DESCRIPTION: &str = "display usage information";
const HELP_JSON_FLAG: &str = "--help-json";
const HELP_JSON_DESCRIPTION: &str = "display usage information encoded in JSON";

/// Returns a `TokenStream` generating a `String` help message.
///
/// Note: `fields` entries with `is_subcommand.is_some()` will be ignored
/// in favor of the `subcommand` argument.
pub(crate) fn help(
    errors: &Errors,
    cmd_name_str_array_ident: &syn::Ident,
    ty_attrs: &TypeAttrs,
    fields: &[StructField<'_>],
    subcommand: Option<&StructField<'_>>,
) -> TokenStream {
    let mut format_lit = "Usage: {command_name}".to_string();

    build_usage_command_line(&mut format_lit, fields, subcommand);

    format_lit.push_str(SECTION_SEPARATOR);

    let description = require_description(errors, Span::call_site(), &ty_attrs.description, "type");
    format_lit.push_str(&description);

    let mut positional = fields.iter().filter(|f| f.kind == FieldKind::Positional).peekable();

    if positional.peek().is_some() {
        format_lit.push_str(SECTION_SEPARATOR);
        format_lit.push_str("Positional Arguments:");
        for arg in positional {
            positional_description(&mut format_lit, arg);
        }
    }

    format_lit.push_str(SECTION_SEPARATOR);
    format_lit.push_str("Options:");
    let options = fields.iter().filter(|f| f.long_name.is_some());
    for option in options {
        option_description(errors, &mut format_lit, option);
    }
    // Also include "help" and "help-json"
    option_description_format(&mut format_lit, None, HELP_FLAG, HELP_DESCRIPTION);
    option_description_format(&mut format_lit, None, HELP_JSON_FLAG, HELP_JSON_DESCRIPTION);

    let subcommand_calculation;
    let subcommand_format_arg;
    if let Some(subcommand) = subcommand {
        format_lit.push_str(SECTION_SEPARATOR);
        format_lit.push_str("Commands:{subcommands}");
        let subcommand_ty = subcommand.ty_without_wrapper;
        subcommand_format_arg = quote! { subcommands = subcommands };
        subcommand_calculation = quote! {
            let subcommands = argh::print_subcommands(
                <#subcommand_ty as argh::SubCommands>::COMMANDS
            );
        };
    } else {
        subcommand_calculation = TokenStream::new();
        subcommand_format_arg = TokenStream::new()
    }

    lits_section(&mut format_lit, "Examples:", &ty_attrs.examples);

    lits_section(&mut format_lit, "Notes:", &ty_attrs.notes);

    if !ty_attrs.error_codes.is_empty() {
        format_lit.push_str(SECTION_SEPARATOR);
        format_lit.push_str("Error codes:");
        for (code, text) in &ty_attrs.error_codes {
            format_lit.push('\n');
            format_lit.push_str(INDENT);
            format_lit.push_str(&format!("{} {}", code, text.value()));
        }
    }

    format_lit.push('\n');

    quote! { {
        #subcommand_calculation
        format!(#format_lit, command_name = #cmd_name_str_array_ident.join(" "), #subcommand_format_arg)
    } }
}

struct OptionHelp {
    short: String,
    long: String,
    description: String,
}

struct PositionalHelp {
    name: String,
    description: String,
}
struct HelpJSON {
    usage: String,
    description: String,
    positional_args: Vec<PositionalHelp>,
    options: Vec<OptionHelp>,
    examples: String,
    notes: String,
    error_codes: Vec<PositionalHelp>,
}

impl HelpJSON {
    fn option_elements_json(&self) -> String {
        let mut retval = String::from("");
        for opt in &self.options {
            if !retval.is_empty() {
                retval.push_str(",\n    ");
            }
            retval.push_str(&format!(
                "{{\"short\": \"{}\", \"long\": \"{}\", \"description\": \"{}\"}}",
                opt.short,
                opt.long,
                escape_json(&opt.description)
            ));
        }
        retval
    }
    fn help_elements_json(elements: &[PositionalHelp]) -> String {
        let mut retval = String::from("");
        for pos in elements {
            if !retval.is_empty() {
                retval.push_str(",\n    ");
            }
            retval.push_str(&format!(
                "{{\"name\": \"{}\", \"description\": \"{}\"}}",
                pos.name,
                escape_json(&pos.description)
            ));
        }
        retval
    }
}

/// Returns a `TokenStream` generating a `String` help message containing JSON.
///
/// Note: `fields` entries with `is_subcommand.is_some()` will be ignored
/// in favor of the `subcommand` argument.
pub(crate) fn help_json(
    errors: &Errors,
    cmd_name_str_array_ident: &syn::Ident,
    ty_attrs: &TypeAttrs,
    fields: &[StructField<'_>],
    subcommand: Option<&StructField<'_>>,
) -> TokenStream {
    let mut usage_format_pattern = "{command_name}".to_string();
    build_usage_command_line(&mut usage_format_pattern, fields, subcommand);

    let mut help_obj = HelpJSON {
        usage: String::from(""),
        description: String::from(""),
        positional_args: vec![],
        options: vec![],
        examples: String::from(""),
        notes: String::from(""),
        error_codes: vec![],
    };

    // Add positional args to the help object.
    let positional = fields.iter().filter(|f| f.kind == FieldKind::Positional);
    for arg in positional {
        let mut description = String::from("");
        if let Some(desc) = &arg.attrs.description {
            description = desc.content.value().trim().to_owned();
        }
        help_obj.positional_args.push(PositionalHelp { name: arg.arg_name(), description });
    }

    // Add options to the help object.
    let options = fields.iter().filter(|f| f.long_name.is_some());
    for option in options {
        let short = match option.attrs.short.as_ref().map(|s| s.value()) {
            Some(c) => String::from(c),
            None => String::from(""),
        };
        let long_with_leading_dashes =
            option.long_name.as_ref().expect("missing long name for option");
        let description =
            require_description(errors, option.name.span(), &option.attrs.description, "field");
        help_obj.options.push(OptionHelp {
            short,
            long: long_with_leading_dashes.to_owned(),
            description,
        });
    }
    // Also include "help" and "help-json"
    help_obj.options.push(OptionHelp {
        short: String::from(""),
        long: String::from(HELP_FLAG),
        description: String::from(HELP_DESCRIPTION),
    });
    help_obj.options.push(OptionHelp {
        short: String::from(""),
        long: String::from(HELP_JSON_FLAG),
        description: String::from(HELP_JSON_DESCRIPTION),
    });

    let subcommand_calculation;
    if let Some(subcommand) = subcommand {
        let subcommand_ty = subcommand.ty_without_wrapper;
        subcommand_calculation = quote! {
            let mut subcommands = String::from("");
            for cmd in  <#subcommand_ty as argh::SubCommands>::COMMANDS {
                if !subcommands.is_empty() {
                    subcommands.push_str(",\n    ");
                }
                subcommands.push_str(&format!("{{\"name\": \"{}\", \"description\": \"{}\"}}",
            cmd.name, cmd.description));
            }
        };
    } else {
        subcommand_calculation = quote! {
            let subcommands = String::from("");
        };
    }

    help_obj.usage = usage_format_pattern.clone();

    help_obj.description =
        require_description(errors, Span::call_site(), &ty_attrs.description, "type");

    let mut example: String = String::from("");
    for lit in &ty_attrs.examples {
        example.push_str(&lit.value());
    }
    help_obj.examples = example;

    let mut note: String = String::from("");
    for lit in &ty_attrs.notes {
        note.push_str(&lit.value());
    }
    help_obj.notes = note;

    if !ty_attrs.error_codes.is_empty() {
        for (code, text) in &ty_attrs.error_codes {
            help_obj.error_codes.push(PositionalHelp {
                name: code.to_string(),
                description: escape_json(&text.value().to_string()),
            });
        }
    }

    let help_options_json = &help_obj.option_elements_json();
    let help_positional_json = HelpJSON::help_elements_json(&help_obj.positional_args);
    let help_error_codes_json = HelpJSON::help_elements_json(&help_obj.error_codes);

    let help_description = escape_json(&help_obj.description);
    let help_examples: TokenStream;
    let help_notes: TokenStream;

    let notes_pattern = escape_json(&help_obj.notes);
    // check if we need to interpolate the string.
    if notes_pattern.contains("{command_name}") {
        help_notes = quote! {
            json_help_string.push_str(&format!(#notes_pattern,command_name = #cmd_name_str_array_ident.join(" ")));
        };
    } else {
        help_notes = quote! {
            json_help_string.push_str(#notes_pattern);
        };
    }
    let examples_pattern = escape_json(&help_obj.examples);
    if examples_pattern.contains("{command_name}") {
        help_examples = quote! {
            json_help_string.push_str(&format!(#examples_pattern,command_name = #cmd_name_str_array_ident.join(" ")));
        };
    } else {
        help_examples = quote! {
            json_help_string.push_str(#examples_pattern);
        };
    }

    quote! {{
        #subcommand_calculation

        // Build up the string for json. The name of the command needs to be dereferenced, so it
        // can't be done in the macro.
        let mut json_help_string = "{\n".to_string();
        let usage_value = format!(#usage_format_pattern,command_name = #cmd_name_str_array_ident.join(" "));
        json_help_string.push_str(&format!("\"usage\": \"{}\",\n",usage_value));
        json_help_string.push_str(&format!("\"description\": \"{}\",\n", #help_description));
        json_help_string.push_str(&format!("\"options\": [{}],\n", #help_options_json));
        json_help_string.push_str(&format!("\"positional\": [{}],\n", #help_positional_json));
        json_help_string.push_str("\"examples\": \"");
        #help_examples;
        json_help_string.push_str("\",\n");
        json_help_string.push_str("\"notes\": \"");
        #help_notes;
        json_help_string.push_str("\",\n");
        json_help_string.push_str(&format!("\"error_codes\": [{}],\n", #help_error_codes_json));
        json_help_string.push_str(&format!("\"subcommands\": [{}]\n", subcommands));
        json_help_string.push_str("}\n");
        json_help_string
    }}
}

/// Escape characters in strings to be JSON compatible.
fn escape_json(value: &str) -> String {
    value.replace("\n", r#"\n"#).replace("\"", r#"\""#)
}

/// A section composed of exactly just the literals provided to the program.
fn lits_section(out: &mut String, heading: &str, lits: &[syn::LitStr]) {
    if !lits.is_empty() {
        out.push_str(SECTION_SEPARATOR);
        out.push_str(heading);
        for lit in lits {
            let value = lit.value();
            for line in value.split('\n') {
                out.push('\n');
                out.push_str(INDENT);
                out.push_str(line);
            }
        }
    }
}

/// Add positional arguments like `[<foo>...]` to a help format string.
fn positional_usage(out: &mut String, field: &StructField<'_>) {
    if !field.optionality.is_required() {
        out.push('[');
    }
    out.push('<');
    let name = field.arg_name();
    out.push_str(&name);
    if field.optionality == Optionality::Repeating {
        out.push_str("...");
    }
    out.push('>');
    if !field.optionality.is_required() {
        out.push(']');
    }
}

/// Add options like `[-f <foo>]` to a help format string.
/// This function must only be called on options (things with `long_name.is_some()`)
fn option_usage(out: &mut String, field: &StructField<'_>) {
    // bookend with `[` and `]` if optional
    if !field.optionality.is_required() {
        out.push('[');
    }

    let long_name = field.long_name.as_ref().expect("missing long name for option");
    if let Some(short) = field.attrs.short.as_ref() {
        out.push('-');
        out.push(short.value());
    } else {
        out.push_str(long_name);
    }

    match field.kind {
        FieldKind::SubCommand | FieldKind::Positional => unreachable!(), // don't have long_name
        FieldKind::Switch => {}
        FieldKind::Option => {
            out.push_str(" <");
            if let Some(arg_name) = &field.attrs.arg_name {
                out.push_str(&arg_name.value());
            } else {
                out.push_str(long_name.trim_start_matches("--"));
            }
            if field.optionality == Optionality::Repeating {
                out.push_str("...");
            }
            out.push('>');
        }
    }

    if !field.optionality.is_required() {
        out.push(']');
    }
}

// TODO(cramertj) make it so this is only called at least once per object so
// as to avoid creating multiple errors.
pub fn require_description(
    errors: &Errors,
    err_span: Span,
    desc: &Option<Description>,
    kind: &str, // the thing being described ("type" or "field"),
) -> String {
    desc.as_ref().map(|d| d.content.value().trim().to_owned()).unwrap_or_else(|| {
        errors.err_span(
            err_span,
            &format!(
                "#[derive(FromArgs)] {} with no description.
Add a doc comment or an `#[argh(description = \"...\")]` attribute.",
                kind
            ),
        );
        "".to_string()
    })
}

/// Describes a positional argument like this:
///  hello       positional argument description
fn positional_description(out: &mut String, field: &StructField<'_>) {
    let field_name = field.arg_name();

    let mut description = String::from("");
    if let Some(desc) = &field.attrs.description {
        description = desc.content.value().trim().to_owned();
    }
    positional_description_format(out, &field_name, &description)
}

fn positional_description_format(out: &mut String, name: &str, description: &str) {
    let info = argh_shared::CommandInfo { name: &*name, description };
    argh_shared::write_description(out, &info);
}

/// Describes an option like this:
///  -f, --force       force, ignore minor errors. This description
///                    is so long that it wraps to the next line.
fn option_description(errors: &Errors, out: &mut String, field: &StructField<'_>) {
    let short = field.attrs.short.as_ref().map(|s| s.value());
    let long_with_leading_dashes = field.long_name.as_ref().expect("missing long name for option");
    let description =
        require_description(errors, field.name.span(), &field.attrs.description, "field");

    option_description_format(out, short, long_with_leading_dashes, &description)
}

fn option_description_format(
    out: &mut String,
    short: Option<char>,
    long_with_leading_dashes: &str,
    description: &str,
) {
    let mut name = String::new();
    if let Some(short) = short {
        name.push('-');
        name.push(short);
        name.push_str(", ");
    }
    name.push_str(long_with_leading_dashes);

    let info = argh_shared::CommandInfo { name: &*name, description };
    argh_shared::write_description(out, &info);
}

/// Builds the usage description command line and appends it to "out".
pub(crate) fn build_usage_command_line(
    out: &mut String,
    fields: &[StructField<'_>],
    subcommand: Option<&StructField<'_>>,
) {
    let positional = fields.iter().filter(|f| f.kind == FieldKind::Positional);
    for arg in positional.clone() {
        out.push(' ');
        positional_usage(out, arg);
    }

    let options = fields.iter().filter(|f| f.long_name.is_some());
    for option in options.clone() {
        out.push(' ');
        option_usage(out, option);
    }

    if let Some(subcommand) = subcommand {
        out.push(' ');
        if !subcommand.optionality.is_required() {
            out.push('[');
        }
        out.push_str("<command>");
        if !subcommand.optionality.is_required() {
            out.push(']');
        }
        out.push_str(" [<args>]");
    }
}
