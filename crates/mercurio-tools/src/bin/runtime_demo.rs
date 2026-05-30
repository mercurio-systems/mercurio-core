use mercurio_core::{ExecutionContext, Runtime, load_model_stack, repo_path};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let example_path = repo_path("test_files/examples/vehicle_model.json");
    let document = load_model_stack(&example_path)?;
    let runtime = Runtime::from_document(document)?;

    let subtypes = runtime.get_subtypes("type.Vehicle")?;
    println!("Subtypes of type.Vehicle: {:?}", subtypes.value);
    println!("Why: {}", runtime.explain(&subtypes));

    let features = runtime.get_features("type.Car")?;
    let model_features = features
        .value
        .iter()
        .filter(|feature| feature.starts_with("feature.") || feature.starts_with("df."))
        .cloned()
        .collect::<Vec<_>>();
    println!("Model features of type.Car: {:?}", model_features);
    println!("Why: {}", runtime.explain(&features));

    let mut context = ExecutionContext::default();
    context.version = 1;
    context.values.insert(
        ("part.engine_left".to_string(), "mass".to_string()),
        json!(120.5),
    );
    context.values.insert(
        ("part.engine_right".to_string(), "mass".to_string()),
        json!(130.0),
    );

    let total_mass = runtime.evaluate("df.totalMass", "assembly.VehicleInstance", &context)?;
    println!("Derived value for df.totalMass: {}", total_mass.value);

    Ok(())
}
