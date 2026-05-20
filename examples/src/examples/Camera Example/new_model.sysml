package VehicleAnalysis {

    import ScalarValues::*;
    import SI::*;

    part def Vehicle {

        attribute mass : MassValue = 1800[kg];
        attribute velocity : Real = 22[m/s];

        // Derived attribute using expression evaluation
        attribute kineticEnergy : EnergyValue =
            0.5 * mass * velocity * 2;

    }

}