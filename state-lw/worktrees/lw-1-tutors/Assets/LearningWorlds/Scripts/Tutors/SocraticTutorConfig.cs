using UnityEngine;

#region Enable Representation of Tutorial System

/// <summary>
/// ScriptableObject resource that contains configurations for each 
/// Convai based Socratic Tutor.
/// Should be created and used for Inspector optimization.
/// </summary>
[CreateAssetMenu(fileName = "SocraticTutorConfig", menuName = "Learning World/SocraticTutorConfig", order = 1)]
[System.Serializable]
public class SocraticTutorConfig : ScriptableObject
{
    [Tooltip("List of configurations for each tutor linked to a Learning World.")]
    public TutorConfig[] tutorConfigs;
    
    [System.Serializable]
    public class TutorConfig
    {
        public SocraticTutorManager.World world;
        public string characterId;
        public Vector3 spawnPosition = Vector3.zero;
        public Vector3 spawnRotation = Vector3.zero;
    }
}

#endregion