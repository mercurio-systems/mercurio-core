package PictureTakingRequirements {

  requirement Req1 {
    id = "REQ-001";
    text = "The system shall focus the camera before shooting.";
  }
  
  requirement Req2 {
    id = "REQ-002";
    text = "The system shall allow shooting only after the focus is set.";
  }
  
  requirement Req321 {
    id = "REQ-003";
    text = "The system shall save the taken picture.";
  }

  part def PowerControlUnit;
  part def InternalCombustionEngine;


  action def ProvidePower {
    action a1;
    action a2;
  }

  allocation a1_to_pcu allocate ProvidePower::a1 to PowerControlUnit;
  allocation a2_to_ice allocate ProvidePower::a2 to InternalCombustionEngine;


  analysis def ThermalAnalysis {

  }

  analysis x1 : ThermalAnalysis;

}