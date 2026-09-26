use super::*;

#[test]
fn form_questions_preserve_field_identity_and_convert_answers_to_native_types() {
    let schema = json!({"type":"object","required":["name","count","confirm"],"properties":{
        "name":{"type":"string","title":"Display name","minLength":2,"maxLength":10},
        "count":{"type":"integer","minimum":1,"maximum":5},
        "confirm":{"type":"boolean"},
        "color":{"type":"string","enum":["blue","red"],"default":"blue"},
        "tags":{"type":"array","items":{"type":"string","enum":["a","b"]},"maxItems":2}}});
    let fields = questions(&schema).unwrap();
    assert!(
        fields
            .iter()
            .any(|field| field["header"] == "name" && field["question"] == "Display name")
    );
    let value = content(
        &schema,
        &json!({"answers":{"name":"Ait","count":"3","confirm":"true","tags":"a, b"}}),
    )
    .unwrap();
    assert_eq!(
        value,
        json!({"name":"Ait","count":3,"confirm":true,"color":"blue","tags":["a","b"]})
    );
    assert_eq!(content(&schema, &json!({"content":value})).unwrap(), value);
    for bad in [
        json!({}),
        json!({"answers":{"name":"x","count":"3","confirm":"true"}}),
        json!({"answers":{"name":"Ait","count":"6","confirm":"true"}}),
        json!({"answers":{"name":"Ait","count":"2.5","confirm":"true"}}),
        json!({"answers":{"name":"Ait","count":"3","confirm":"yes"}}),
        json!({"answers":{"name":"Ait","count":"3","confirm":"true","injected":"x"}}),
    ] {
        assert!(content(&schema, &bad).is_err(), "{bad}");
    }
}

#[test]
fn unsupported_nested_forms_are_rejected_and_empty_forms_need_no_fields() {
    assert_eq!(
        content(&json!({"type":"object"}), &Value::Null).unwrap(),
        json!({})
    );
    for schema in [
        json!({}),
        json!({"type":"object","required":["missing"]}),
        json!({"type":"object","properties":{"a":{"type":"object"}}}),
        json!({"type":"object","properties":{"a":{"type":"string","pattern":".*"}}}),
        json!({"type":"object","properties":{"a":{"type":"string","enum":[true]}}}),
    ] {
        assert!(questions(&schema).is_err());
    }
}
