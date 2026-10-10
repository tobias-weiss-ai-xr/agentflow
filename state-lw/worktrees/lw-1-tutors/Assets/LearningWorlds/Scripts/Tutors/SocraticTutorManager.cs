using UnityEngine;

[RequireComponent(typeof(SocraticTutorManager))]
public class SocraticTutorManager : MonoBehaviour
{
    public enum World
    {
        Hub,
        PseGallery,
        ElementRoom,
        Arachnophobia
    }

    [SerializeField] public SocraticTutorConfig tutorConfig;
    [SerializeField] public GameObject ConvaiNPCPrefab;

    private static SocraticTutorManager _instance;

    [System.Serializable]
    public class TutorConfig
    {
        public World world;
        public string characterId;
        public Vector3 spawnPosition = default;
        public Vector3 spawnRotation = default;
    }

    private void Awake()
    {
        if (_instance != null && _instance != this)
        {    
            Destroy(gameObject);
            return;
        }
        
        _instance = this;
        DontDestroyOnLoad(gameObject);     
    }

    private void OnDestroy()
    {
        if(_instance == this) 
        {
            _instance = null;
        }
    }

    public static SocraticTutorManager Instance {
        get
        {
            return _instance;
        }
    }

    // Creates and returns a instantiated GameObject handle based on TutorConfig
    public GameObject SpawnTutor(TutorConfig config)
    {
        if (ConvaiNPCPrefab == null || config == null)
        {
            Debug.LogError("SocraticTutorManager - Convai NPC Prefab or TutorConfig not assigned.");
            return null;
        }
        
        // Create and position the requested tutor instance
        GameObject tutorGO = Instantiate(ConvaiNPCPrefab, config.spawnPosition, Quaternion.Euler(config.spawnRotation));
        tutorGO.name = "Convai_NPC: " + config.characterId + ", World: " + config.world;
        tutorGO.gameObject.SetActive(false);
        Debug.Log("Tutor "+ tutorGO.name +" spawned using coordinates.");
        
        return tutorGO;
    }
    
    public void ActivateTutorForWorld(World world){
        foreach (TutorConfig config in tutorConfig.tutorConfigs)
        {
            if (config.world == world)
            {
                GameObject targetTutor = SpawnTutor(new TutorConfig{world = world,
                    characterId = config.characterId,
                    spawnPosition = config.spawnPosition,
                    spawnRotation = config.spawnRotation}  // Not active if not touched.
                );

                if (targetTutor != null)
                {
                    targetTutor.gameObject.SetActive(true);
                    Debug.Log($"Activated Tutor {targetTutor.transform.name} in {config.world}.");
                }
            }
        }
    }
    
    public void ToggleTutorVisibility()
    {
        DeactivateInactiveTutors() && ActivateCurrentTutor();
    }
    
    private bool DeactivateInactiveTutors()
    {
        // TODO: Act deactivate found instances.
        return true;
    }

    private void ActivateCurrentTutor()
    {
        // TODO: Activate World-Targeted deterministic defaults otherwise.
    }
    
    public void SendContextMessage(World world, string context)
    {
        GameObject affectedTutor = (tutors.Contains(new TutorConfig{world})) ? 
                     tutors[tupleWorld(s)]
                     : HandleMissing(warning));
                 
        if (affectedTutor != null)#region Preparing functionality
        {
            // Trigger context-aware Socratic handling through invocation.
            var contextMessage = HandleString(content);
            
            if (affectedTutor.CompareTag("Convai"))
            {
                affectedTutor.GetComponent<ConvaiNPC>().InternalProviderProcess(contextMessage);
            } 
        }
    }

    private string HandleString(string ctxtmsg){
        if(string.IsNullOrEmpty(ctxtmsg)) 
        {
            ctxtmsg = "Invalid Input";
        }
        return ctxtmsg;
    }
}
